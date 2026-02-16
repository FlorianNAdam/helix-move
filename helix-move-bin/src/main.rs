use clap::Parser;
use helix_move_lib::{
    add_missing_directories, build_rules, create_edit_rules, filter_full_rules,
    normalize_rules, EditRule,
};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use tempfile::{Builder, TempDir};

#[derive(Parser)]
struct Args {
    /// Directory whose files should be listed
    dir: PathBuf,

    /// Path to the LSP binary (defaults to workspace build)
    #[arg(long)]
    lsp: Option<PathBuf>,

    /// Path to the helix binary (defaults to hx)
    #[arg(long)]
    helix_bin: Option<PathBuf>,

    /// Show what would happen without moving files
    #[arg(long)]
    dry_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct LanguagesToml {
    language: Vec<Language>,
    language_server: HashMap<String, LanguageServer>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Language {
    name: String,
    scope: String,
    file_types: Vec<FileType>,
    roots: Vec<String>,
    language_servers: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct FileType {
    glob: String,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct LanguageServer {
    command: String,
    args: Vec<String>,
    config: InitOptions,
}

#[derive(Serialize)]
struct InitOptions {
    file_list_file: String,
    files: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ConfigToml {
    editor: Editor,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Editor {
    lsp: Lsp,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Lsp {
    display_inlay_hints: bool,
}

//
// ============================
// Helpers
// ============================
//

pub fn collect_paths(root: impl AsRef<Path>) -> anyhow::Result<Vec<String>> {
    let root = root.as_ref().canonicalize()?;
    let mut result = Vec::new();

    fn visit_dir(
        dir: &Path,
        root: &Path,
        acc: &mut Vec<String>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            let rel = path.strip_prefix(root).unwrap();
            let mut rel_string = rel.to_string_lossy().replace('\\', "/");

            let metadata = fs::symlink_metadata(&path)?;
            let file_type = metadata.file_type();

            if file_type.is_dir() {
                rel_string.push('/');
                acc.push(rel_string.clone());
                visit_dir(&path, root, acc)?;
            } else {
                acc.push(rel_string);
            }
        }
        Ok(())
    }

    visit_dir(&root, &root, &mut result)?;

    result.sort();

    Ok(result)
}

fn resolve_lsp_path(args: &Args) -> anyhow::Result<PathBuf> {
    if let Some(path) = &args.lsp {
        return Ok(path.clone());
    }

    let current_exe = std::env::current_exe()?;
    let exe_dir = current_exe
        .parent()
        .ok_or(anyhow::anyhow!("Failed to determine executable directory"))?;

    let lsp_name = if cfg!(windows) {
        "helix-move-lsp.exe"
    } else {
        "helix-move-lsp"
    };

    Ok(exe_dir.join(lsp_name))
}

fn resolve_helix_path(args: &Args) -> anyhow::Result<PathBuf> {
    if let Some(helix_bin) = &args.helix_bin {
        Ok(helix_bin.clone())
    } else {
        Ok(PathBuf::from_str("hx")?)
    }
}

fn confirm() -> anyhow::Result<bool> {
    print!("\nApply these changes? [Y/n]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let input = input.trim().to_lowercase();
    Ok(input != "n" && input != "no")
}

//
// ============================
// Main
// ============================
//

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if !args.dir.is_dir() {
        anyhow::bail!("Provided path is not a directory");
    }

    let lsp_path = resolve_lsp_path(&args)?;
    let helix_bin_path = resolve_helix_path(&args)?;

    let temp_dir: TempDir = Builder::new()
        .prefix("helix-move.")
        .tempdir_in("/tmp")?;

    let base_path = temp_dir.path();
    let helix_path = base_path.join(".helix");
    fs::create_dir_all(&helix_path)?;

    // ---- Collect original entries (FILES + DIRECTORIES) ----
    let original_entries: Vec<String> = collect_paths(&args.dir)?;

    let list_file = base_path.join("file-list");
    fs::write(&list_file, original_entries.join("\n"))?;

    // ---- Generate languages.toml ----
    let mut language_servers = HashMap::new();
    language_servers.insert(
        "hello-lsp".to_string(),
        LanguageServer {
            command: lsp_path
                .canonicalize()?
                .display()
                .to_string(),
            args: vec![],
            config: InitOptions {
                file_list_file: list_file.display().to_string(),
                files: original_entries.clone(),
            },
        },
    );

    let languages = LanguagesToml {
        language: vec![Language {
            name: "file-list".to_string(),
            scope: "source.file-list".to_string(),
            file_types: vec![FileType {
                glob: "file-list".to_string(),
            }],
            roots: vec![],
            language_servers: vec!["hello-lsp".to_string()],
        }],
        language_server: language_servers,
    };

    fs::write(
        helix_path.join("languages.toml"),
        toml::to_string_pretty(&languages)?,
    )?;

    let config = ConfigToml {
        editor: Editor {
            lsp: Lsp {
                display_inlay_hints: true,
            },
        },
    };

    fs::write(
        helix_path.join("config.toml"),
        toml::to_string_pretty(&config)?,
    )?;

    // ---- Launch Helix ----
    Command::new(helix_bin_path)
        .current_dir(base_path)
        .arg("-v")
        .arg("file-list")
        .status()?;

    // ---- Read edited file ----
    let edited_content = fs::read_to_string(&list_file)?;
    let edited_entries: Vec<String> = edited_content
        .lines()
        .map(|l| l.to_string())
        .collect();

    // Phase 1
    let rules = build_rules(&original_entries, &edited_entries);

    // Phase 2
    let normalized = normalize_rules(&rules);

    // Phase 3
    let full_rules = add_missing_directories(&normalized);

    let filtered_rules = filter_full_rules(&full_rules);

    if filtered_rules.is_empty() {
        println!("No Changes");
        return Ok(());
    }

    println!("Changes:");
    for rule in filtered_rules {
        println!("{}", rule);
    }

    if !confirm()? {
        println!("Cancelled");
        return Ok(());
    }

    // Phase 4
    let edit_rules = create_edit_rules(&full_rules);

    build_and_replace(&args.dir, &edit_rules)?;

    println!("Applied successfully");
    Ok(())
}

fn build_and_replace(
    root: &Path,
    edit_rules: &[EditRule],
) -> anyhow::Result<()> {
    let root = root.canonicalize()?;

    let parent = root
        .parent()
        .ok_or(anyhow::anyhow!("Invalid root"))?;

    let temp_dir: TempDir = Builder::new()
        .prefix(".helix-move-builder.")
        .tempdir_in(parent)?;

    for rule in edit_rules {
        rule.apply(&root, temp_dir.path())?;
    }

    let temp_dir_path = temp_dir.keep();

    replace_contents(&temp_dir_path, &root)?;

    Ok(())
}

fn replace_contents(src: &Path, dst: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(dst)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        fs::rename(from, to)?;
    }

    fs::remove_dir(src)?;

    Ok(())
}
