use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process,
};

use serde_json::Value;
use syn::{Item, ItemEnum};

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| "Usage: idl-gen <source-file>".to_owned())?;
    let dependencies = find_path_dependencies(&path);
    let dep_dirs = dependencies.values().cloned().collect::<Vec<_>>();
    let idl = spel_framework_core::idl_gen::generate_idl_from_file_with_deps(&path, &dep_dirs)
        .map_err(|error| error.to_string())?;

    // spel-framework emits the top-level `types` array in HashMap iteration
    // order, which is non-deterministic across processes. Sort it by name so
    // regenerated IDL is byte-stable regardless of where it runs.
    let mut value = serde_json::to_value(&idl)
        .map_err(|error| format!("converting IDL to JSON value failed: {error}"))?;
    add_external_instruction_variant_indices(&mut value, &dependencies)?;
    sort_idl_types(&mut value);
    let json = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("serializing IDL JSON failed: {error}"))?;
    println!("{json}");
    Ok(())
}

fn sort_idl_types(value: &mut Value) {
    if let Some(types) = value.get_mut("types").and_then(Value::as_array_mut) {
        types.sort_by(|left, right| {
            left.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .cmp(
                    right
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        });
    }
}

fn add_external_instruction_variant_indices(
    idl: &mut Value,
    dependencies: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let Some(instruction_type) = idl.get("instruction_type") else {
        return Ok(());
    };
    let instruction_type = instruction_type
        .as_str()
        .ok_or_else(|| "IDL instruction_type must be a string".to_owned())?
        .to_owned();
    let (crate_name, enum_name) = external_instruction_type_parts(&instruction_type)?;
    let dependency_dir = dependencies.get(crate_name).ok_or_else(|| {
        format!(
            "external instruction_type `{instruction_type}` does not resolve to a path dependency named `{crate_name}`"
        )
    })?;
    let enum_source_path = dependency_dir.join("src/lib.rs");
    let enum_source = fs::read_to_string(&enum_source_path).map_err(|error| {
        format!(
            "reading external instruction enum source `{}` failed: {error}",
            enum_source_path.display()
        )
    })?;
    let variant_indices = instruction_variant_indices_from_source(&enum_source, enum_name)?;
    let instructions = idl
        .get_mut("instructions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "IDL has no instructions array".to_owned())?;
    add_instruction_variant_indices(instructions, &variant_indices, &instruction_type)
}

fn external_instruction_type_parts(instruction_type: &str) -> Result<(&str, &str), String> {
    let segments = instruction_type
        .split("::")
        .map(str::trim)
        .collect::<Vec<_>>();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!(
            "external instruction_type `{instruction_type}` must use a `crate::Enum` path"
        ));
    }
    let crate_name = segments
        .first()
        .copied()
        .ok_or_else(|| "external instruction type has no crate segment".to_owned())?;
    let enum_name = segments
        .last()
        .copied()
        .ok_or_else(|| "external instruction type has no enum segment".to_owned())?;
    Ok((crate_name, enum_name))
}

fn instruction_variant_indices_from_source(
    source: &str,
    enum_name: &str,
) -> Result<BTreeMap<String, (String, u32)>, String> {
    let file = syn::parse_file(source)
        .map_err(|error| format!("parsing external instruction enum source failed: {error}"))?;
    let instruction_enum = find_named_enum(&file.items, enum_name)
        .ok_or_else(|| format!("external instruction enum `{enum_name}` was not found"))?;
    if instruction_enum.variants.is_empty() {
        return Err(format!(
            "external instruction enum `{enum_name}` has no variants"
        ));
    }

    let mut indices = BTreeMap::new();
    for (position, variant) in instruction_enum.variants.iter().enumerate() {
        let variant_index = u32::try_from(position).map_err(|_| {
            format!("external instruction enum `{enum_name}` has too many variants")
        })?;
        let variant_name = variant.ident.to_string();
        let normalized_name = normalized_identifier(&variant_name)?;
        if let Some((previous_name, _)) =
            indices.insert(normalized_name, (variant_name.clone(), variant_index))
        {
            return Err(format!(
                "external instruction enum `{enum_name}` has ambiguous variants `{previous_name}` and `{variant_name}`"
            ));
        }
    }
    Ok(indices)
}

fn find_named_enum<'a>(items: &'a [Item], enum_name: &str) -> Option<&'a ItemEnum> {
    for item in items {
        if let Item::Enum(item_enum) = item {
            if item_enum.ident == enum_name {
                return Some(item_enum);
            }
            continue;
        }
        if let Item::Mod(item_mod) = item {
            if let Some((_, nested_items)) = &item_mod.content {
                if let Some(item_enum) = find_named_enum(nested_items, enum_name) {
                    return Some(item_enum);
                }
            }
        }
    }
    None
}

fn add_instruction_variant_indices(
    instructions: &mut [Value],
    variant_indices: &BTreeMap<String, (String, u32)>,
    instruction_type: &str,
) -> Result<(), String> {
    if instructions.len() != variant_indices.len() {
        return Err(format!(
            "external instruction_type `{instruction_type}` has {} enum variants but {} IDL instructions; generation must expose a complete mapping",
            variant_indices.len(),
            instructions.len()
        ));
    }

    let mut used_variants = BTreeMap::new();
    for (row_index, instruction) in instructions.iter_mut().enumerate() {
        let object = instruction
            .as_object_mut()
            .ok_or_else(|| format!("IDL instruction row {row_index} must be a JSON object"))?;
        let instruction_name = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("IDL instruction row {row_index} has no name"))?
            .to_owned();
        let normalized_name = normalized_identifier(&instruction_name)?;
        let (enum_name, variant_index) = variant_indices.get(&normalized_name).ok_or_else(|| {
            format!(
                "IDL instruction `{instruction_name}` does not match a variant of external instruction_type `{instruction_type}`"
            )
        })?;
        if let Some(previous_instruction) =
            used_variants.insert(normalized_name, instruction_name.clone())
        {
            return Err(format!(
                "IDL instructions `{previous_instruction}` and `{instruction_name}` both map to external enum variant `{enum_name}`"
            ));
        }

        let expected = Value::from(*variant_index);
        if let Some(existing) = object.get("variant_index") {
            if existing != &expected {
                return Err(format!(
                    "IDL instruction `{instruction_name}` declares variant_index {existing}, but external enum variant `{enum_name}` has index {variant_index}"
                ));
            }
        }
        object.insert("variant_index".to_owned(), expected);
    }

    if used_variants.len() != variant_indices.len() {
        return Err(format!(
            "external instruction_type `{instruction_type}` did not produce a complete variant map"
        ));
    }
    Ok(())
}

fn normalized_identifier(value: &str) -> Result<String, String> {
    let value = value.strip_prefix("r#").unwrap_or(value);
    let normalized = value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized.is_empty() {
        return Err(format!(
            "identifier `{value}` has no alphanumeric characters"
        ));
    }
    Ok(normalized)
}

/// Return local path dependencies by their Cargo dependency name.
fn find_path_dependencies(source_path: &Path) -> BTreeMap<String, PathBuf> {
    (|| -> Option<BTreeMap<String, PathBuf>> {
        let manifest = find_crate_manifest(source_path)?;
        let content = fs::read_to_string(&manifest).ok()?;
        let value = toml::from_str::<toml::Value>(&content).ok()?;
        let manifest_dir = manifest.parent()?;

        let mut dependencies = BTreeMap::new();
        if let Some(table) = value.get("dependencies").and_then(toml::Value::as_table) {
            for (name, dependency) in table {
                let Some(relative_path) = dependency.get("path").and_then(toml::Value::as_str)
                else {
                    continue;
                };
                let dependency_dir = manifest_dir.join(relative_path);
                if dependency_dir.is_dir() {
                    dependencies.insert(name.clone(), dependency_dir);
                }
            }
        }
        Some(dependencies)
    })()
    .unwrap_or_default()
}

/// Walk up from `start` to find the nearest `Cargo.toml`.
fn find_crate_manifest(start: &Path) -> Option<PathBuf> {
    let mut directory = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    loop {
        let candidate = directory.join("Cargo.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        directory = directory.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{add_instruction_variant_indices, instruction_variant_indices_from_source};

    #[test]
    fn external_enum_order_overrides_handler_order() -> Result<(), String> {
        let enum_source = r#"
            pub enum Instruction {
                Transfer,
                PrintNft,
                SetAuthority,
            }
        "#;
        let variants = instruction_variant_indices_from_source(enum_source, "Instruction")?;
        let mut instructions = vec![
            json!({"name": "transfer"}),
            json!({"name": "set_authority"}),
            json!({"name": "print_nft"}),
        ];

        add_instruction_variant_indices(&mut instructions, &variants, "token_core::Instruction")?;

        let indices = instructions
            .iter()
            .map(|instruction| instruction.get("variant_index").and_then(Value::as_u64))
            .collect::<Vec<_>>();
        assert_eq!(indices, vec![Some(0), Some(2), Some(1)]);
        Ok(())
    }

    #[test]
    fn external_enum_mapping_requires_every_variant() -> Result<(), String> {
        let variants = instruction_variant_indices_from_source(
            "pub enum Instruction { Transfer, PrintNft }",
            "Instruction",
        )?;
        let mut instructions = vec![json!({"name": "transfer"})];

        let error = add_instruction_variant_indices(
            &mut instructions,
            &variants,
            "token_core::Instruction",
        )
        .err()
        .ok_or_else(|| "incomplete mapping unexpectedly succeeded".to_owned())?;
        assert!(error.contains("complete mapping"));
        Ok(())
    }

    #[test]
    fn token_idl_matches_the_token_core_wire_order() -> Result<(), String> {
        let variants = instruction_variant_indices_from_source(
            include_str!("../../../programs/token/core/src/lib.rs"),
            "Instruction",
        )?;
        let mut idl =
            serde_json::from_str::<Value>(include_str!("../../../artifacts/token-idl.json"))
                .map_err(|error| format!("parsing token IDL fixture failed: {error}"))?;
        let instructions = idl
            .get_mut("instructions")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "token IDL fixture has no instructions array".to_owned())?;
        add_instruction_variant_indices(instructions, &variants, "token_core::Instruction")?;

        for (name, expected_index) in [
            ("transfer", 0),
            ("print_nft", 7),
            ("set_authority", 8),
            ("set_authority_with_authority", 9),
        ] {
            let actual_index = instructions
                .iter()
                .find(|instruction| instruction.get("name").and_then(Value::as_str) == Some(name))
                .and_then(|instruction| instruction.get("variant_index"))
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("token IDL fixture has no variant index for `{name}`"))?;
            assert_eq!(actual_index, expected_index);
        }
        Ok(())
    }
}
