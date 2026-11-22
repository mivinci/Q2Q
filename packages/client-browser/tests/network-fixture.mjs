import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer, connect } from 'node:net';
import { once } from 'node:events';
import { runtime } from '../../../apps/track/tests/runtime-fixture.ts';
const root=fileURLToPath(new URL('../../../',import.meta.url));
const execute=promisify(execFile);
export async function networkFixture() {
  const temporary=await mkdtemp(join(tmpdir(),'q2q-browser-network-'));
  const reserve=createServer();reserve.listen(0,'127.0.0.1');await once(reserve,'listening');
  const port=reserve.address().port;await new Promise(resolve=>reserve.close(resolve));
  const config=join(temporary,'punch.toml');
  await writeFile(config,`bind = "127.0.0.1:${port}"\npunch_id = "browser-test"\nwebsocket_tls = false\n[identity]\ncert = ${JSON.stringify(join(temporary,'cert.pem'))}\nkey = ${JSON.stringify(join(temporary,'key.pem'))}\n`);
  let child;let worker;let proxy;
  const proxySockets=new Set();
  async function closeProxy(){for(const socket of proxySockets)socket.destroy();if(proxy?.listening)await new Promise(resolve=>proxy.close(resolve));}
  async function stopPunch() {
    if(!child||child.exitCode!==null||child.signalCode!==null)return;
    const stopped=once(child,'exit');child.kill('SIGTERM');const force=setTimeout(()=>child.kill('SIGKILL'),2000);
    try{await stopped;}finally{clearTimeout(force);}
  }
  try {
    const binary=join(root,'target/debug/q2q-punch-server');
    const result=await execute(binary,[config,'--init-identity'],{cwd:root});
    const public_key=result.stdout.trim();
    if(Buffer.from(public_key,'base64').length!==32)throw Error('invalid test Punch identity');
    async function startPunch(){
    child=spawn(binary,[config],{cwd:root,env:{...process.env,RUST_LOG:'info'},stdio:['ignore','pipe','pipe']});
    await new Promise((resolve,reject)=>{
      let output='';const timer=setTimeout(()=>reject(Error('test Punch startup timeout')),10000);
      const read=bytes=>{output=(output+bytes.toString()).slice(-4096);if(output.includes('listeners ready')){clearTimeout(timer);resolve();}};
      child.stdout.on('data',read);child.stderr.on('data',read);
      child.once('exit',code=>{clearTimeout(timer);reject(Error(`test Punch exited ${code}`));});child.once('error',reject);
    });
    }
    await startPunch();
    // A distinct TCP entry with a path/query catches clients that still guess
    // a WebSocket URL from the QUIC authority. Only signalling crosses this proxy.
    proxy=createServer(socket=>{
      const upstream=connect({host:'127.0.0.1',port});
      for(const value of [socket,upstream]){proxySockets.add(value);value.on('close',()=>proxySockets.delete(value));value.on('error',()=>{socket.destroy();upstream.destroy();});}
      socket.pipe(upstream);upstream.pipe(socket);
    });
    proxy.listen(0,'127.0.0.1');await once(proxy,'listening');
    const punch={punch_id:'browser-test',endpoints: [`quic://127.0.0.1:${port}`,`ws://127.0.0.1:${proxy.address().port}/signal/punch?region=local`],public_key};
    worker=await runtime('5000',punch);
    const trackUrl=String(await worker.ready).replace(/\/$/,'');
    return {trackUrl,punch,stopPunch,async restartPunch(){await stopPunch();await startPunch();},async close(){await Promise.allSettled([worker.dispose(),stopPunch()]);await closeProxy();await rm(temporary,{recursive:true,force:true});}};
  }catch(error){await worker?.dispose();await stopPunch();await closeProxy();await rm(temporary,{recursive:true,force:true});throw error;}
}
