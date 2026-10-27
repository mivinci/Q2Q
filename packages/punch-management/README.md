# @q2q/punch-management

Private Admin/Registry RPC schemas and canonical request fingerprints. Public discovery remains in `@q2q/punch-discovery`; management is exposed through a separate named Worker entrypoint, never a new JWT scope or public HTTP route.

Policy requests identify the original administrator, node, desired state, expected revision, reason and UUID. Fingerprints bind the administrator subject and canonical intent; email is display metadata. Responses validate authoritative before/after state and operation identity. Admin persists intent before RPC and reconciles uncertain outcomes by request ID.
