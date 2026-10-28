# Lower-Mac HIL vocabulary

`oer-ieee80211-conformance-control` is the host-safe, `no_std` HIL lower-mac vocabulary. It depends on the generic HIL message/frame contract; the framework does not depend on it. Its `registry` feature exposes descriptors and `messages.lock` for host composition. Image services advertise their mounted keys through the agent.
