//! YAML binding example: format a date string.
//!
//! Demonstrates loading a module from a YAML binding definition.
//! Pair with `format_date.binding.yaml` in the same directory.

use apcore::BindingLoader;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut loader = BindingLoader::new();

    // Load the binding definition
    let binding_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("bindings")
        .join("format_date.binding.yaml");

    loader.load_from_yaml(&binding_path)?;

    println!("Registered bindings:");
    for name in loader.list_bindings() {
        println!("  - {name}");
        if let Ok(def) = loader.resolve(name) {
            println!(
                "    module: {}  callable: {}",
                def.target.module_name, def.target.callable
            );
        }
    }

    Ok(())
}
