use std::fmt::Write as _;
use std::path::Path;

fn main() {
    emit_component_audit();
    compile_ui();
}

/// Compiles `ui/main.slint`, with element debug info when the `mcp` feature
/// asks for it.
///
/// Slint documents `SLINT_EMIT_DEBUG_INFO=1` for this, but an environment
/// variable is a second switch to remember and forget: without the debug info
/// every MCP tool that names an element fails at runtime, long after the
/// build. Tying it to the feature makes one flag mean one thing. It is not
/// free -- toggling it recompiles the whole generated module -- which is why
/// it stays off unless the MCP server is being compiled in too.
fn compile_ui() {
    if std::env::var_os("CARGO_FEATURE_MCP").is_some() {
        let config = slint_build::CompilerConfiguration::new().with_debug_info(true);
        slint_build::compile_with_config("ui/main.slint", config)
            .expect("Slint compilation failed");
    } else {
        slint_build::compile("ui/main.slint").expect("Slint compilation failed");
    }
}

/// Records every `export component` in `ui/` so the mockup tool can subtract
/// its own catalog from it and show the difference as an UNCATALOGUED group.
/// Scanned here rather than at runtime because the `.slint` sources are not
/// shipped, and doing it at build time means a widget written this morning is
/// listed as a to-do this afternoon without anyone maintaining a second list.
fn emit_component_audit() {
    println!("cargo:rerun-if-changed=ui");

    let mut entries: Vec<(String, String)> = Vec::new();
    let mut modules: Vec<String> = Vec::new();
    for entry in std::fs::read_dir("ui").expect("ui directory") {
        let path = entry.expect("ui entry").path();
        if path
            .extension()
            .is_some_and(|extension| extension == "slint")
        {
            modules.push(
                path.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    modules.sort();

    for module in &modules {
        // slint-build already emits a rerun key for every file main.slint
        // reaches, but a new module is not reachable until it is imported.
        println!("cargo:rerun-if-changed=ui/{module}");
        let source = std::fs::read_to_string(Path::new("ui").join(module)).expect("read module");
        for line in source.lines() {
            let Some(rest) = line.strip_prefix("export component ") else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                entries.push((name, module.clone()));
            }
        }
    }
    entries.sort();

    let mut generated = String::from(
        "/// Every component exported from a `ui/*.slint` module, as\n\
         /// `(component, module)`, sorted by component name. Written by\n\
         /// `build.rs`; see `mockup::uncatalogued`.\n\
         pub const EXPORTED_COMPONENTS: &[(&str, &str)] = &[\n",
    );
    for (name, module) in &entries {
        writeln!(generated, "    ({name:?}, {module:?}),").expect("format audit table");
    }
    generated.push_str("];\n");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(Path::new(&out_dir).join("mockup_exports.rs"), generated)
        .expect("write mockup_exports.rs");
}
