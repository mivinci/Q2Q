# @q2q/accounts

Shared platform account/session access checks. This package has no OAuth, UI or router dependency.

`prepareAccountSession` reads the user's current enabled state and server-owned session version. Platform's Better Auth hook translates `AccountAccessError` to its 403 API error. `authorizeAccountSession` independently checks the persisted session, user status, version and expiry before assigning request identity.

`sessionExpirySql` is used by authorization and Admin's valid-session count. It supports Better Auth's ISO SQLite timestamps and legacy epoch-millisecond rows, including exact expiry boundaries. Its alias is an internal SQL fragment, never request input.

Database enforcement and defaults are provided by platform migration `0019_user_access.sql`. The Admin transaction advances the version and deletes sessions alongside account state and audit changes; this library never grants access from a cached user object.
