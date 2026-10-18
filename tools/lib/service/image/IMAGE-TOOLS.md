# Images

Image packages share one directory, grouped by their contracts.

| Path | Owns |
| --- | --- |
| [pipeline](pipeline/README.md) (`oer-tool-service-image`) | `build(ImageSpec) -> ImageBundle`, boot flows, build exclusion and source inputs |
| `bundle/` (`oer-tool-service-image-bundle`) | The bundle format, verified snapshots and flash segments; device writers consume it without the build pipeline |
| `encode/` (`oer-tool-service-image-encode`) | Build-time ESP image encoding and stage-two packing |
| `policy/` (`oer-tool-data-image-policy`) | Stack policy and compiler limits |
| `checks/` (`oer-tool-service-image-checks`) | Optional pipeline checks assembled for a caller |
| `check/interrupts/`, `check/placement/`, `check/stack/` | Individual image audits over the shared ELF and RISC-V analysis layers |
| `compare/` (`oer-tool-service-image-compare`) | Function comparison modulo placement |
| `linker/` (`oer-tool-service-image-linker`) | Link-input checks followed by the target's `rust-lld` |

The pipeline compiles, checks and encodes each image. Device operations
write the bundle's verified bytes without encoding them again. The bundle
and encoder belong to the foundation host layer; the pipeline, checks,
comparison, policy and linker belong to the build host layer.

ESP-IDF environment and catalog support stays in [`../esp-idf/`](../esp-idf).
