use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

fn to_kebab_case(s: &str) -> String {
    let mut kebab = String::new();
    for (i, c) in s.char_indices() {
        if c.is_uppercase() {
            if i > 0 {
                kebab.push('-');
            }
            kebab.extend(c.to_lowercase());
        } else {
            kebab.push(c);
        }
    }
    kebab
}

fn generate_api_config(
    proto_files: &[PathBuf],
    out_dir: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config_path = out_dir.join("api_config.yaml");
    let mut file = File::create(&config_path)?;

    writeln!(file, "type: google.api.Service")?;
    writeln!(file, "config_version: 3")?;
    writeln!(file, "\nhttp:")?;
    writeln!(file, "  rules:")?;

    for proto_path in proto_files {
        let proto_file = File::open(proto_path)?;
        let reader = BufReader::new(proto_file);

        let mut current_package = String::new();
        let mut current_service = String::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();

            if trimmed.starts_with("package ") {
                if let Some(pkg) = trimmed.strip_prefix("package ") {
                    current_package = pkg.trim_end_matches(';').to_string();
                }
            } else if trimmed.starts_with("service ") {
                if let Some(srv) = trimmed.strip_prefix("service ") {
                    current_service = srv
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('{')
                        .to_string();
                }
            } else if trimmed.starts_with("rpc ") {
                if let Some(rpc_part) = trimmed.strip_prefix("rpc ") {
                    let rpc_name = rpc_part.split_whitespace().next().unwrap_or("").to_string();
                    if !current_package.is_empty()
                        && !current_service.is_empty()
                        && !rpc_name.is_empty()
                    {
                        writeln!(
                            file,
                            "    - selector: {}.{}.{}",
                            current_package, current_service, rpc_name
                        )?;
                        writeln!(
                            file,
                            "      post: /{}/{}",
                            current_package,
                            to_kebab_case(&rpc_name)
                        )?;
                        writeln!(file, "      body: \"*\"")?;
                    }
                }
            }
        }
    }

    Ok(config_path)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_docs_dir = PathBuf::from("./api-docs");
    if api_docs_dir.exists() {
        fs::remove_dir_all(&api_docs_dir)?;
    }
    fs::create_dir_all(&api_docs_dir)?;

    // Tell cargo to rerun if the proto folder changes
    println!("cargo:rerun-if-changed=proto");

    let mut proto_files = Vec::new();
    for entry in fs::read_dir("proto")? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "proto") {
            proto_files.push(path);
        }
    }

    let out_dir = env::var("OUT_DIR")?;
    let api_config_path = generate_api_config(&proto_files, Path::new(&out_dir))?;

    tonic_build::configure()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Tells protoc to use the openapiv2 plugin and output to our specific directory
        .protoc_arg("--openapiv2_out=./api-docs")
        .protoc_arg(format!(
            "--openapiv2_opt=grpc_api_configuration={}",
            api_config_path.display()
        ))
        .compile_protos(&proto_files, &[PathBuf::from("proto")])?;

    Ok(())
}
