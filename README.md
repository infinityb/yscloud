# Cross Machine Application Traffic / Outgoing Traffic

Solution #1:

Before application traffic, we can do a handshake to "dial" the correct application behind a `Runtime`.  After the dial is successful, we can pass the connection file descriptor directly to application. The application can then set up the connection normally, with SSL if it desires.

Solution #2:

Exposing a SOCKS5 socket inside containers might be applicable for both internet and internal destinations.

Solution #3 (best?):

Exposing a socket which is backed by an extension of sni-multiplexor which can do dialing based on the SNI name found in the ClientHello.  This implements us something like SOCKS5 but for cheap and is exclusively service-name based.

# Outgoing Traffic

Applications must dial through the runtime - regular socket creation will be forbidden.  The destination may be an internet destination or an internal cloud destination, if allowed by the security policy of the application.   

# Cross Application Traffic (same machine)

If the destination is an internal cloud destination and the remote application is running on the same runtime, the OS network stack will be bypassed and unix domain sockets will be used.  Some services may need a remote to be running on the same machine - We can probably get away with having a connection option for this.  Use cases: memory buffer exchange, logging aggregators, security helpers (e.g. identity signer?), local object store caching (e.g. a file-system privileged helper which will handle fetching and caching per-machine), ...?

# Platform intrinsic services

`yscloud.identity-signer` formerly `org.yshi.internal_certificate_issuer`.  Need to spec out better.  Include in the yscloud-linker executable for now?  Need to support seamless restart of intrinsics.


# Logging causality

Application libraries? Do we even need it at this layer?  Probably should end up as an intrinsic service.

# In-place version upgrade

If an application supports it, the runtime may send an upgrade message with an included file descriptor.  This file descriptors remote end will be a new instance of the application.  The new upgrade-compatible application will deserialize its state and OS resources from the descriptor and take over execution for the previous application.  This is probably a very-future feature and we probably shouldn't consider it at this time.

In some cases this might not be required, e.g. HTTP servers.  We can have an old and new version of a stateless server running concurrently - the new instance will get all the new traffic and the old instance will have the opportunity to finish serving inflight requests before terminating.  We can implement this much more easily, so this should be our initial target.

## Glossary

### Platform Intrinsic Service

A service type that does not need to be explicitly defined in the deployment manifest, causing it to be  unpinned.  This will allow us to upgrade core infrastructure (like logging, debugging and identity signing) without users needing to ship completely new deployments.

### Short-lived runtime certificate

Short-lived runtime certificates (SLRC) are certificates valid for a short period of time, issued by `yscloud.identity-signer`, for the use of authenticating to other components in the cloud including ones owned by other users, if they allow it.

### Highwire

A system that handles connecting to internal, hosted components (e.g. the database cluster in yyc1, or redis).  For postgresql, it will inject authentication information based on the incoming client certificate.

### Request-triggered Service Activation

Similar to Lambdas on AWS, unscoped.

