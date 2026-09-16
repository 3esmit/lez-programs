use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use risc0_binfmt::ProgramBinary;

fn main() -> Result<()> {
    let mut args = env::args_os();
    let command = args
        .next()
        .unwrap_or_else(|| OsString::from("risc0-packager"));
    let first_argument = required_arg(&mut args, &command, "command or guest ELF")?;

    if first_argument == OsStr::new("validate") {
        let binary = required_arg(&mut args, &command, "program .bin")?;
        if args.next().is_some() {
            bail!(
                "usage: {} validate <program-bin>",
                command.to_string_lossy()
            );
        }

        return validate_file(&binary);
    }

    if first_argument == OsStr::new("image-id") {
        let binary = required_arg(&mut args, &command, "program .bin")?;
        if args.next().is_some() {
            bail!(
                "usage: {} image-id <program-bin>",
                command.to_string_lossy()
            );
        }

        return print_image_id(&binary);
    }

    let output = required_arg(&mut args, &command, "output .bin")?;

    if args.next().is_some() {
        bail!(
            "usage: {} <guest-elf> <output-bin>",
            command.to_string_lossy()
        );
    }

    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let user_elf = fs::read(&first_argument)
        .with_context(|| format!("failed to read {}", first_argument.display()))?;
    let kernel_elf = risc0_build::GuestOptions::default().kernel();
    let binary = ProgramBinary::new(&user_elf, &kernel_elf);

    fs::write(&output, binary.encode())
        .with_context(|| format!("failed to write {}", output.display()))?;

    Ok(())
}

fn validate_file(path: &Path) -> Result<()> {
    let encoded = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    validate_program_binary(&encoded)
        .with_context(|| format!("invalid RISC Zero program binary: {}", path.display()))
}

fn print_image_id(path: &Path) -> Result<()> {
    let encoded = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let binary = ProgramBinary::decode(&encoded)
        .with_context(|| format!("cannot decode ProgramBinary: {}", path.display()))?;
    let image_id = binary.compute_image_id().with_context(|| {
        format!(
            "ProgramBinary does not contain valid RISC Zero ELFs: {}",
            path.display()
        )
    })?;
    println!("{image_id}");
    Ok(())
}

fn validate_program_binary(encoded: &[u8]) -> Result<()> {
    let binary = ProgramBinary::decode(encoded).context("cannot decode ProgramBinary")?;
    binary
        .compute_image_id()
        .context("ProgramBinary does not contain valid RISC Zero ELFs")?;
    Ok(())
}

fn required_arg(
    args: &mut impl Iterator<Item = OsString>,
    command: &OsString,
    name: &str,
) -> Result<PathBuf> {
    match args.next() {
        Some(value) => Ok(PathBuf::from(value)),
        None => bail!(
            "missing {name}; usage: {} <guest-elf> <output-bin>",
            command.to_string_lossy()
        ),
    }
}

#[cfg(test)]
mod tests {
    use risc0_binfmt::ProgramBinary;

    use super::validate_program_binary;

    #[test]
    fn rejects_magic_valid_garbage() {
        assert!(validate_program_binary(b"R0BFgarbage").is_err());
    }

    #[test]
    fn rejects_decodable_binary_with_invalid_elfs() {
        let encoded = ProgramBinary::new(&[1, 2, 3, 4], &[5, 6, 7, 8]).encode();

        assert!(ProgramBinary::decode(&encoded).is_ok());
        assert!(validate_program_binary(&encoded).is_err());
    }
}
