# 23. Reusable SDK HTTP transport

Status: accepted for implementation on 2026-09-05.

The operator-approved workspace startup improvement adds a credential-free RemoteEndpoint
factory. It validates one exact HTTPS origin, loads bounded trust roots and keeps the
configured DNS server identity. Calling connect with an access-token provider constructs
an actor-bound client and performs the existing machine/contract handshake.

RemoteEndpoint clones share a bounded HTTP/1.1 connection pool. A request obtains fresh
authority from its own provider and attaches sensitive authorization headers to that
request only. Completed bounded responses may return a connection to the pool; canceled,
failed, expired-idle or closed connections are discarded. No mutation is replayed merely
because a pooled connection fails. Existing durable operation recovery owns that decision.

The pool admits at most 16 concurrent HTTP requests and retires idle connections after
30 seconds. Contract headers are verified before consuming a response body, whose existing
four-MiB bound remains. WebSocket transports use dedicated TLS connections, preserving
connection-bound session proofs and independent lifetimes.

Workspace may retain RemoteEndpoint as deployment transport configuration. It must not
retain an actor's token provider, credentials, or resolved grants between requests.
The public Client builder remains compatible. No wire contract or frozen bundle changes.
