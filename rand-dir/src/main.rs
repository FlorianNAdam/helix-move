use clap::Parser;
use rand::{rngs::StdRng, Rng, RngExt, SeedableRng};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Root directory where the random tree will be created
    #[arg(long)]
    root: PathBuf,

    /// Maximum depth of directory nesting
    #[arg(long, default_value_t = 3)]
    max_depth: usize,

    /// Maximum number of subdirectories per directory
    #[arg(long, default_value_t = 3)]
    max_branch: usize,

    /// Maximum number of leaf files per directory
    #[arg(long, default_value_t = 2)]
    max_leaves: usize,

    /// Optional seed for deterministic generation
    #[arg(long)]
    seed: Option<u64>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.max_depth == 0 {
        return Err("max_depth must be > 0".into());
    }

    let mut rng: StdRng = match args.seed {
        Some(seed) => StdRng::seed_from_u64(seed),
        None => {
            let mut thread_rng = rand::rng();
            StdRng::from_rng(&mut thread_rng)
        }
    };

    fs::create_dir_all(&args.root)?;

    create_random_tree(
        &args.root,
        1,
        args.max_depth,
        args.max_branch,
        args.max_leaves,
        &mut rng,
    )?;

    println!("Random directory tree created at: {}", args.root.display());

    Ok(())
}

fn create_random_tree<R: Rng>(
    current: &Path,
    depth: usize,
    max_depth: usize,
    max_branch: usize,
    max_leaves: usize,
    rng: &mut R,
) -> Result<(), Box<dyn std::error::Error>> {
    if depth > max_depth {
        return Ok(());
    }

    // ---- Create leaf files (local per node) ----
    let leaf_count = rng.random_range(0..=max_leaves);

    for i in 0..leaf_count {
        let file_name = format!("f{}_{}", depth, i);
        let file_path = current.join(file_name);
        File::create(file_path)?;
    }

    // ---- Create subdirectories ----
    let branch_count = rng.random_range(0..=max_branch);

    for i in 0..branch_count {
        let dir_name = format!("d{}_{}", depth, i);
        let new_dir = current.join(dir_name);
        fs::create_dir_all(&new_dir)?;

        create_random_tree(
            &new_dir,
            depth + 1,
            max_depth,
            max_branch,
            max_leaves,
            rng,
        )?;
    }

    Ok(())
}
