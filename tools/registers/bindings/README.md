# Register binding index

`oer-registers-bindings` is the format of a publication's binding index,
`chips/<chip>/registers/published/*.bindings.toml`: every register address with its
fields and, when the publication generates a PAC, the PAC path that reaches it.
`cargo registers generate` writes it; tools that name addresses and bits read
it through `BindingIndex` instead of declaring its shape again. It is a foundation data
format, so every host layer may read it.
