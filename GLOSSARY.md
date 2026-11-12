# libharness

A Rust library that embeds an LLM agent harness in a host application. The host drives it; the library never reaches the outside world on its own.

## Language

**Host**:
The Rust application that embeds libharness, registers capabilities and starts sessions.
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
Anything the host registers for sessions to use: a tool, skill, plugin or option, or the model call itself.
_Avoid_: Provider, hook, integration

**Session**:
One independent run started by a prompt, with its own turn loop, ending naturally or when the host ends it.
_Avoid_: Run, conversation, thread

**Loop**:
The code that repeatedly makes the model call and the tool calls and feeds the results back into the core until a session ends. The host writes it, registers it as a capability, or uses the one the library optionally ships.
_Avoid_: Driver, runner, orchestrator

**State**:
The owned, serialisable data that goes into and comes out of every core call; the core keeps nothing between calls.
_Avoid_: Context, memory, snapshot

**Adapter**:
An optional piece outside the core that turns an outside service into a capability, such as the OpenRouter adapter.
_Avoid_: Provider layer, driver, connector
