# HIL protocol

`oer-protocols-hil-protocol` is the radio-independent message and frame skeleton: keys, `Message`/`Endpoint`, `messages!`, bounded framing and the envelope. `base` owns discovery, image-key pages, boot/post-mortem evidence, link health and request admission (a device answers an observation at once and works on one command of the image's at a time: a second command that arrives meanwhile is answered `Rejected(Busy)`).

- [Wire contract](wire.md): framing, version checks, discovery and boot identity.

Each message's key hashes its path and postcard schema. A semantic change with the same schema requires a new path. `messages!` emits typed endpoints/topics/properties and their compile-time `KEYS`. There are no radio-family features or dependencies.

A changed path or schema changes the key, so an image and a scenario built from different vocabularies disagree on the keys the image announces, which the host checks before any case. Domain crates depend on the framework, never the reverse; the framework knows no other owner's messages.

An image's advertised keys come from its mounted [services](../agent/README.md#image-keys). A scenario requires the keys of the services it uses; unrelated extra keys remain accepted. Properties are unit markers and never sent as frames. A refusal is `base::Rejected`; an endpoint has exactly one typed response, which may follow related events.
