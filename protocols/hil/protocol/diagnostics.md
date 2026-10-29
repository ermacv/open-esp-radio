# HIL diagnostics

## Requested reset

`lifecycle::RequestReset` is served by an image that mounts the agent's
lifecycle service: the image acknowledges with `ResetAccepted`, waits until
the acknowledgement's frame was written and resets through its retained
path; the next boot reports the reason.

## Stack measurements

`system::StackUsage` carries task watermarks and optional dedicated IRQ watermarks for
both harts for the messages of other modules that report it. A hart that runs no
executor reports `None`. Images with shared task/IRQ stacks report `None` for
both dedicated IRQ fields.

IRQ sampling runs in thread mode with local interrupts masked during the SRAM
scan. It therefore adds a short interruption to RF servicing; these diagnostic
runs are not a timing baseline with instrumentation removed. The image's stack
policy supplies the nonzero required reserve. These are observed boot-lifetime
watermarks, not worst-case bounds or evidence of every possible nested IRQ.
