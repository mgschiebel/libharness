# libharness

A Rust library that embeds an LLM agent harness in a host application. The host drives it; the library never reaches the outside world on its own.

## Language

**Host**:
The Rust application that embeds libharness, registers capabilities and decides how many times to call the core and when to stop.
_Avoid_: App, client, caller, embedder

**Library**:
libharness as a whole: the core plus everything shipped with it.
_Avoid_: Framework, SDK, harness (when meaning the whole product)

**Core**:
The layer of the library that performs no I/O of its own: every part that sits under the no-I/O gate, not any single crate.
_Avoid_: Engine, kernel

**Codec**:
A piece of the core that translates between the library's model types and one service's wire format, performing no I/O.
_Avoid_: Provider, driver, parser

**Capability**:
Anything the host registers for the library to call: a tool, skill, plugin or option, or the model call itself.
_Avoid_: Provider, hook, integration

**Loop**:
The code that repeatedly makes the model call and the tool calls and feeds the results back into the core, until it stops. The host writes its own or uses the one the library optionally ships. The library has no notion of a run or session that ends: stopping is the loop's choice, and for a host-written loop it is the host not calling again.
_Avoid_: Session, run, driver, runner, orchestrator

**State**:
The owned, serialisable data, message history included, that goes into and comes out of every core call. The host can read and build it; the core keeps nothing between calls and only appends to the history.
_Avoid_: Context, memory, snapshot

**Transport**:
The capability that carries one HTTP request, as data, to a model service and returns the status, the headers and a stream of bytes. The host registers it; the library ships an optional one outside the core.
_Avoid_: HTTP client, connector, adapter

**Exchange**:
The core code that runs one model call over a Transport: it sends the codec's request, decodes the reply and maps failures to the library's error.
_Avoid_: Driver, client, adapter

**Offloader**:
The capability that runs a callback which blocks on a thread of the host's choosing, so that it cannot stall the run's other callbacks or the host's own thread. The host registers it; the library ships an optional one outside the core.
_Avoid_: Runner, thread pool, executor
