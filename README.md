# Cross Machine Application Traffic

Solution #1:

Before application traffic, we can do a handshake to "dial" the correct application behind a `Runtime`.  After the dial is successful, we can pass the connection file descriptor directly to application. The application can then set up the connection normally, even with SSL if it desires.

# Outgoing Traffic

Applications must dial through the runtime - regular socket creation will be forbidden.  The destination may be an internet destination or an internal cloud destination, if allowed by the security policy of the application.

# Cross Application Traffic (same machine)

If the destination is an internal cloud destination and the remote application is running on the same runtime, the OS network stack will be bypassed and unix domain sockets will be used.  Some services may need a remote to be running on the same machine - We can probably get away with having a connection option for this.  Use cases: memory buffer exchange, logging aggregators, security helpers (e.g. identity signer?), local object store caching (e.g. a file-system privileged helper which will handle fetching and caching per-machine), ...?

# Logging causality

Application libraries? Do we even need it at this layer?

# Socket Activation

???

# In-place version upgrade

If an application supports it, the runtime may send an upgrade message with an included file descriptor.  This file descriptors remote end will be a new instance of the application.  The new upgrade-compatible application will deserialize its state and OS resources from the descriptor and take over execution for the previous application.

In some cases this might not be required, e.g. HTTP servers.  We can have an old and new version of a stateless server running concurrently - the new instance will get all the new traffic and the old instance will have the opportunity to finish serving traffic before terminating.

This is probably a very-future feature and we probably shouldn't consider it at this time.

# braindrumps

Per-datacenter replicated services (for bookkeeping and cross-DC state sync)
Application migration across machines - need the in-place version upgrade facility (working cross-machine) and remote TCP session holders (with session resume) if hitting internet traffic.

Sorry - that thing with my name was kind of an over-reaction. It's fine when no one's around.

# Examples?

## Services likely to be immediately portable

### https://aibi.yshi.org
- service dependency declarations
  - remote object store service
  - local object store service
- local object store caching
  - we can serve it on a linode, backed by homelab
- cross-machine application traffic

```
Frontend (linode)
|-> WebApp (linode)
|   |-> Caching Local Object Store (linode)
|   \-> File Server (yyc)
|       \-> Authoritative Local Object Store (yyc)
\-> Local Logger Aggregator (linode)
    \-> Log Storage (yyc)
```

## Services potentially portable

### https://xn--9h8h.yshi.org
- cross-machine application traffic
- service dependency declarations
  - database helper

```
Frontend (linode)
|-> WebApp (linode)
|   |-> xn--9h8h database helper (yyc)
|   |   \-> PostgreSQL
|   \-> Local Logger Aggregator (yyc)
|       \-> Log Storage (yyc)
\-> Local Logger Aggregator (linode)
    \-> Log Storage (yyc)
```

### nano
```
nano
|-> internet connections
|-> xn--9h8h database helper
\-> Local Logger Aggregator
    \-> Log Storage
```

### fourchan-spider

This one's tricky - we probably want to port the whole toolchain?  How do we declare persistent services that don't depend on any real events?  Do we just run a wrapper in systemd?  Terminating the wrapper terminates the program.


```
spider
|-> internet connections
|-> DBFrame Storage Server
\-> Local Logger Aggregator
    \-> Log Storage
```

maybe an observer writing to the dbframe files can simply ask for a stream of pages from the spider daemon.  We want the spider to be globally singleton though - so we still have some considerations to work out here.

```
observer (*n) [real process, writes to dbframe files]
\-> spider
    \-> Local Logger Aggregator
        \-> Log Storage
```
