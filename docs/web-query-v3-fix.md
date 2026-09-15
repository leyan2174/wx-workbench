# Web query pipe compatibility

The contacts, sessions and tag-members HTTP routes previously sent a bare
Request and decoded a bare Response. The daemon now requires QueryHello,
QueryEnvelope and QueryReply, so those routes failed against the real daemon.

Web's query adapter now reuses service/query_client's connect_query,
write_query and decode_query_response, including OS peer verification,
protocol/runtime matching and the shared bounded frame reader. There is no
legacy fallback, daemon startup, request replay or blocking worker bridge.
No shared client interface was changed.

The existing RuntimeContext remains pinned by the Web host. HTTP origin,
token and CSRF checks are unchanged. Query slot acquisition still has a
two-second bound; the complete request has its existing 20-second timeout
(one second for Ping) and eight-MiB response limit. Business failure checks
and HTTP projections are unchanged. History continues to use Web RPC.

The new runtime_isolation test
daemon_tasks::web_query::three_web_query_routes_use_real_daemon_v3_and_stay_account_bound
starts the real wx daemon and Web processes against two synthetic encrypted
accounts. It checks actual contact/session/tag data, wire display projections,
pagination, HTTP token rejection, invalid input, cross-account isolation and
connect-only behavior after explicit daemon shutdown. The fixtures do not mock
query responses or use real account data.

Validation pending: the parent agent must run this test and the existing Web
query queue/router tests during unified validation. No Cargo command was run
while implementing this slice.
