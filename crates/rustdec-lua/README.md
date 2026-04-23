# rustdec-plugin

Lua scripting engine for RustDec.

Exposes named **hooks** — pipeline extension points where a Lua script can
observe or modify data at each stage of the analysis.

> **Status: V1 stub.**  The public API is declared and the Lua VM is
> initialised, but no script is executed and no hook fires.  The goal for
> this release is to validate the build chain and reserve the interfaces.
> Full execution is planned for V2.

## Intended usage (V2)

```rust
use rustdec_plugin::{Hook, PluginEngine};

let mut engine = PluginEngine::new()?;
engine.load_file("plugins/rename_funcs.lua")?;
engine.load_string(r#"
    function on_function_lifted(func)
        if func.name:match("^sub_") then
            func.name = "unknown_" .. func.entry_addr
        end
    end
"#, "inline")?;

engine.call_hook(Hook::OnFunctionLifted)?;
```

## Hook points

Hooks map to specific moments in the analysis pipeline:

| Hook | Fired when |
|---|---|
| `OnBinaryLoaded` | A binary has been parsed into a `BinaryObject` |
| `OnFunctionDetected` | A function entry point has been found |
| `OnCfgBuilt` | The CFG of a function has been built |
| `OnFunctionLifted` | A function has been translated to SSA IR |
| `OnCodeEmitted` | Pseudo-code for a function has been generated |
| `OnAnalysisComplete` | The full binary analysis is finished |

## API

```rust
// Create a new engine (initialises the Lua VM)
let mut engine = PluginEngine::new()?;

// Load a script from disk
engine.load_file("path/to/script.lua")?;

// Load a script from a string
engine.load_string(source, "chunk_name")?;

// Fire a hook (V1: no-op; V2: calls registered Lua handlers)
engine.call_hook(Hook::OnFunctionLifted)?;

// How many scripts are currently loaded
let n = engine.loaded_count();
```

## Error handling

```rust
pub enum PluginError {
    Lua(mlua::Error),           // Lua runtime error
    NotFound(String),           // script file not found
    HookFailed(String, String), // hook returned an error value
}
```

## Dependencies

- [`mlua`](https://crates.io/crates/mlua) — safe Lua 5.4 bindings (vendored build, no system Lua required)
