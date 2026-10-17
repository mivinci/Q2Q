//! Shared Track state for several HTTP instances backed by one Redis primary.
//! The announce and session updates are atomic Redis scripts. This backend is
//! deliberately for a single primary: its announce spans several Redis keys.

use crate::state::{
  Candidate, InfoHash, OnlineRecord, PunchAssignment, StateError, TrackState, UnixSeconds,
};
use q2q_core::identity::PeerId;
use redis::aio::ConnectionManager;

const ANNOUNCE: &str = r#"
local peer = ARGV[1]
local expiry = ARGV[6]
local prefix = ARGV[9]
redis.call('HSET', KEYS[1], 'pk', ARGV[2], 'sk', ARGV[3], 'seq', ARGV[4], 'punch', ARGV[5], 'expiry', expiry)
redis.call('EXPIREAT', KEYS[1], expiry)
local at = 10
for _ = 1, tonumber(ARGV[7]) do
  local hash = ARGV[at]
  at = at + 1
  redis.call('ZREM', prefix .. 'file:' .. hash .. ':members', peer)
  redis.call('HDEL', prefix .. 'file:' .. hash .. ':meta', peer)
  redis.call('ZREM', prefix .. 'holding-expiry', hash .. ':' .. peer)
end
for _ = 1, tonumber(ARGV[8]) do
  local hash = ARGV[at]
  local complete = ARGV[at + 1]
  at = at + 2
  redis.call('ZADD', prefix .. 'file:' .. hash .. ':members', 0, peer)
  redis.call('HSET', prefix .. 'file:' .. hash .. ':meta', peer, expiry .. ':' .. complete)
  redis.call('ZADD', prefix .. 'holding-expiry', expiry, hash .. ':' .. peer)
end
return 1
"#;

const ADVANCE: &str = r#"
local current = redis.call('HMGET', KEYS[1], 'sk', 'seq', 'expiry')
if not current[1] or current[1] ~= ARGV[1] or tonumber(current[3]) <= tonumber(ARGV[4]) then
  return 0
end
local old = current[2]
local next = ARGV[2]
if #next < #old or (#next == #old and next <= old) then
  return 1
end
redis.call('HSET', KEYS[1], 'seq', next)
if ARGV[3] ~= '' then
  redis.call('HSET', KEYS[1], 'expiry', ARGV[3])
  redis.call('EXPIREAT', KEYS[1], ARGV[3])
end
return 2
"#;

const SWEEP: &str = r#"
local members = redis.call('ZRANGE', KEYS[1], '-inf', ARGV[1], 'BYSCORE', 'LIMIT', 0, ARGV[2])
local prefix = ARGV[3]
for _, member in ipairs(members) do
  local cut = string.find(member, ':', 1, true)
  local hash = string.sub(member, 1, cut - 1)
  local peer = string.sub(member, cut + 1)
  local meta_key = prefix .. 'file:' .. hash .. ':meta'
  local value = redis.call('HGET', meta_key, peer)
  if value then
    local colon = string.find(value, ':', 1, true)
    if tonumber(string.sub(value, 1, colon - 1)) <= tonumber(ARGV[1]) then
      redis.call('HDEL', meta_key, peer)
      redis.call('ZREM', prefix .. 'file:' .. hash .. ':members', peer)
    end
  end
  redis.call('ZREM', KEYS[1], member)
end
return #members
"#;

#[derive(Clone)]
pub struct RedisState {
  conn: ConnectionManager,
  prefix: String,
}

type StoredOnline = (
  Option<Vec<u8>>,
  Option<Vec<u8>>,
  Option<u64>,
  Option<String>,
  Option<u64>,
);

impl RedisState {
  pub async fn connect(url: &str) -> redis::RedisResult<Self> {
    Self::connect_with_prefix(url, "q2q:").await
  }

  /// Use a distinct prefix when several Q2Q deployments share one Redis DB.
  pub async fn connect_with_prefix(url: &str, prefix: &str) -> redis::RedisResult<Self> {
    let client = redis::Client::open(url)?;
    Ok(Self {
      conn: client.get_connection_manager().await?,
      prefix: prefix.to_owned(),
    })
  }
}

fn storage(e: redis::RedisError) -> StateError {
  StateError::Unavailable(e.to_string())
}

fn malformed(what: &str) -> StateError {
  StateError::Unavailable(format!("malformed Redis {what}"))
}

fn hex(hash: &InfoHash) -> String {
  hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn online_key(prefix: &str, peer: &str) -> String {
  format!("{prefix}online:{peer}")
}

fn file_members(prefix: &str, hash: &InfoHash) -> String {
  format!("{prefix}file:{}:members", hex(hash))
}

fn file_meta(prefix: &str, hash: &InfoHash) -> String {
  format!("{prefix}file:{}:meta", hex(hash))
}

fn holding(value: &str) -> Result<(UnixSeconds, bool), StateError> {
  let (expiry, complete) = value.split_once(':').ok_or_else(|| malformed("holding"))?;
  let expiry = expiry.parse().map_err(|_| malformed("holding expiry"))?;
  let complete = match complete {
    "0" => false,
    "1" => true,
    _ => return Err(malformed("holding complete flag")),
  };
  Ok((expiry, complete))
}

impl TrackState for RedisState {
  async fn announce(
    &self,
    record: OnlineRecord,
    add: &[InfoHash],
    del: &[InfoHash],
    complete: &[InfoHash],
    _ttl: UnixSeconds,
  ) -> Result<(), StateError> {
    let mut cmd = redis::cmd("EVAL");
    cmd
      .arg(ANNOUNCE)
      .arg(1)
      .arg(online_key(&self.prefix, record.peer_id.as_str()))
      .arg(record.peer_id.as_str())
      .arg(record.public_key.as_slice())
      .arg(record.session_key.as_slice())
      .arg(record.last_seq)
      .arg(&record.punch.punch_id)
      .arg(record.expires_at)
      .arg(del.len())
      .arg(add.len())
      .arg(&self.prefix);
    for h in del {
      cmd.arg(hex(h));
    }
    for h in add {
      cmd
        .arg(hex(h))
        .arg(if complete.contains(h) { 1 } else { 0 });
    }
    let _: i64 = cmd
      .query_async(&mut self.conn.clone())
      .await
      .map_err(storage)?;
    Ok(())
  }

  async fn online(
    &self,
    peer_id: &PeerId,
    now: UnixSeconds,
  ) -> Result<Option<OnlineRecord>, StateError> {
    let fields: StoredOnline = redis::cmd("HMGET")
      .arg(online_key(&self.prefix, peer_id.as_str()))
      .arg(&["pk", "sk", "seq", "punch", "expiry"])
      .query_async(&mut self.conn.clone())
      .await
      .map_err(storage)?;
    if fields.0.is_none()
      && fields.1.is_none()
      && fields.2.is_none()
      && fields.3.is_none()
      && fields.4.is_none()
    {
      return Ok(None);
    }
    let (Some(pk), Some(sk), Some(seq), Some(punch), Some(expiry)) = fields else {
      return Err(malformed("online record"));
    };
    if expiry <= now {
      return Ok(None);
    }
    let public_key = pk.try_into().map_err(|_| malformed("public key"))?;
    let session_key = sk.try_into().map_err(|_| malformed("session key"))?;
    Ok(Some(OnlineRecord {
      peer_id: peer_id.clone(),
      public_key,
      session_key,
      last_seq: seq,
      punch: PunchAssignment {
        punch_id: punch,
        url: String::new(),
        public_key: Vec::new(),
      },
      expires_at: expiry,
    }))
  }

  async fn advance_session(
    &self,
    peer_id: &PeerId,
    session_key: &[u8; 32],
    seq: u64,
    renew_until: Option<UnixSeconds>,
    now: UnixSeconds,
  ) -> Result<(), StateError> {
    let result: i64 = redis::cmd("EVAL")
      .arg(ADVANCE)
      .arg(1)
      .arg(online_key(&self.prefix, peer_id.as_str()))
      .arg(session_key.as_slice())
      .arg(seq)
      .arg(renew_until.map(|v| v.to_string()).unwrap_or_default())
      .arg(now)
      .query_async(&mut self.conn.clone())
      .await
      .map_err(storage)?;
    match result {
      0 => Err(StateError::UnknownSession),
      1 => Err(StateError::Replayed(seq)),
      2 => Ok(()),
      _ => Err(malformed("advance result")),
    }
  }

  async fn query(
    &self,
    info_hash: &InfoHash,
    limit: usize,
    after: Option<&PeerId>,
    now: UnixSeconds,
  ) -> Result<Vec<Candidate>, StateError> {
    let members = file_members(&self.prefix, info_hash);
    let meta = file_meta(&self.prefix, info_hash);
    let mut cursor = after.map(|p| p.as_str().to_string());
    let mut out = Vec::with_capacity(limit);
    while out.len() < limit {
      let start = cursor
        .as_ref()
        .map(|s| format!("({s}"))
        .unwrap_or_else(|| "-".into());
      let ids: Vec<String> = redis::cmd("ZRANGE")
        .arg(&members)
        .arg(start)
        .arg("+")
        .arg("BYLEX")
        .arg("LIMIT")
        .arg(0)
        .arg(128)
        .query_async(&mut self.conn.clone())
        .await
        .map_err(storage)?;
      if ids.is_empty() {
        break;
      }
      cursor = ids.last().cloned();
      let mut pipe = redis::pipe();
      for id in &ids {
        pipe.cmd("HGET").arg(&meta).arg(id);
        pipe
          .cmd("HMGET")
          .arg(online_key(&self.prefix, id))
          .arg(&["pk", "punch", "expiry"]);
      }
      let values: Vec<redis::Value> = pipe
        .query_async(&mut self.conn.clone())
        .await
        .map_err(storage)?;
      for (index, id) in ids.iter().enumerate() {
        let value: Option<String> = redis::from_redis_value(values[index * 2].clone())
          .map_err(|e| StateError::Unavailable(e.to_string()))?;
        let Some(value) = value else { continue };
        let (expiry, complete) = holding(&value)?;
        if expiry <= now {
          continue;
        }
        let online: (Option<Vec<u8>>, Option<String>, Option<u64>) =
          redis::from_redis_value(values[index * 2 + 1].clone())
            .map_err(|e| StateError::Unavailable(e.to_string()))?;
        let (Some(pk), Some(punch_id), Some(online_expiry)) = online else {
          continue;
        };
        if online_expiry <= now {
          continue;
        }
        out.push(Candidate {
          peer_id: PeerId::unchecked(id),
          public_key: pk.try_into().map_err(|_| malformed("public key"))?,
          punch_id,
          complete,
        });
        if out.len() == limit {
          break;
        }
      }
      if ids.len() < 128 {
        break;
      }
    }
    Ok(out)
  }

  async fn sweep(&self, now: UnixSeconds) -> Result<usize, StateError> {
    let mut total = 0;
    loop {
      let count: usize = redis::cmd("EVAL")
        .arg(SWEEP)
        .arg(1)
        .arg(format!("{}holding-expiry", self.prefix))
        .arg(now)
        .arg(256)
        .arg(&self.prefix)
        .query_async(&mut self.conn.clone())
        .await
        .map_err(storage)?;
      total += count;
      if count < 256 {
        return Ok(total);
      }
    }
  }
}
