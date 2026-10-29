# Lower-Mac HIL vocabulary

`oer-ieee80211-conformance-control` is the host-safe, `no_std` HIL lower-mac vocabulary. It depends on the generic HIL message/frame contract; the framework does not depend on it. Its `registry` feature exposes descriptors and `messages.lock` for host composition. Image services advertise their mounted keys through the agent.

Hardware observation payloads carry event counts, not encoded MPDU snapshots or publication journals. EDCA comparison reports publication/table/record/draw counts and returned owners; rollback comparison reports the observed kind and quarantine. Fault records and control injections are independent.
