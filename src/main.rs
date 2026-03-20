mod types;
mod providers;

use std::fs;
use std::path::Path;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;

use providers::archive::ArchiveProvider;
use providers::gutenberg::GutenbergProvider;
use providers::libgen::LibgenProvider;
use providers::BookProvider;
use types::BookResult;

#[derive(Parser)]
#[command(name = "bookfinder", about = "Search and download books from multiple sources")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Search for books across providers
    Search {
        /// Search query
        query: String,

        /// Provider to search (libgen, gutenberg, archive, or all)
        #[arg(short, long, default_value = "all")]
        provider: ProviderChoice,

        /// Maximum number of results per provider
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Download a book by provider and ID
    Download {
        /// Provider name (libgen, gutenberg, or archive)
        provider: String,

        /// Book ID from search results
        id: String,

        /// Output directory
        #[arg(short, long, default_value = ".")]
        output: String,
    },
}

#[derive(Clone, ValueEnum)]
enum ProviderChoice {
    Libgen,
    Gutenberg,
    Archive,
    All,
}

fn get_provider(name: &str) -> Result<Box<dyn BookProvider>> {
    match name {
        "libgen" => Ok(Box::new(LibgenProvider::new())),
        "gutenberg" => Ok(Box::new(GutenbergProvider::new())),
        "archive" => Ok(Box::new(ArchiveProvider::new())),
        _ => bail!("Unknown provider: {}. Use libgen, gutenberg, or archive.", name),
    }
}

fn get_providers(choice: &ProviderChoice) -> Vec<Box<dyn BookProvider>> {
    match choice {
        ProviderChoice::Libgen => vec![Box::new(LibgenProvider::new())],
        ProviderChoice::Gutenberg => vec![Box::new(GutenbergProvider::new())],
        ProviderChoice::Archive => vec![Box::new(ArchiveProvider::new())],
        ProviderChoice::All => vec![
            Box::new(LibgenProvider::new()),
            Box::new(GutenbergProvider::new()),
            Box::new(ArchiveProvider::new()),
        ],
    }
}

fn print_results(results: &[BookResult]) {
    for (i, book) in results.iter().enumerate() {
        println!(
            "[{}] [{}] [{}] {} - {} ({})",
            i + 1,
            book.provider.cyan(),
            book.format.yellow(),
            book.title.bold().white(),
            book.author,
            book.size,
        );
        println!("    ID: {}", book.download_id);
    }
}

fn do_search(query: &str, provider_choice: &ProviderChoice, limit: usize) -> Result<()> {
    let providers = get_providers(provider_choice);
    let mut all_results: Vec<BookResult> = Vec::new();

    for p in &providers {
        println!("Searching {}...", p.name().cyan());
        match p.search(query) {
            Ok(mut results) => {
                results.truncate(limit);
                all_results.append(&mut results);
            }
            Err(e) => {
                eprintln!("{}: {} - {}", "Error".red(), p.name(), e);
            }
        }
    }

    if all_results.is_empty() {
        println!("No results found.");
    } else {
        println!();
        print_results(&all_results);
        println!("\n{} result(s) found.", all_results.len());
    }

    Ok(())
}

fn do_download(provider_name: &str, id: &str, output_dir: &str) -> Result<()> {
    let provider = get_provider(provider_name)?;
    let output_path = Path::new(output_dir);

    if !output_path.exists() {
        fs::create_dir_all(output_path)?;
        println!("Created output directory: {}", output_dir);
    }

    let book = BookResult {
        id: id.to_string(),
        title: id.to_string(),
        author: String::new(),
        format: String::new(),
        size: String::new(),
        provider: provider_name.to_string(),
        download_id: id.to_string(),
    };

    println!("Downloading from {}...", provider_name.cyan());

    let file_path = provider.download(&book, output_path)?;
    let metadata = fs::metadata(&file_path)?;
    let size_kb = metadata.len() as f64 / 1024.0;

    println!(
        "{} Downloaded to {} ({:.1} KB)",
        "Success!".green().bold(),
        file_path.display(),
        size_kb,
    );

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Search {
            query,
            provider,
            limit,
        } => do_search(query, provider, *limit),
        Commands::Download {
            provider,
            id,
            output,
        } => do_download(provider, id, output),
    }
}
