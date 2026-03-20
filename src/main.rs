use std::fs;
use std::path::Path;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use bookfinder::providers::annas_archive::AnnasArchiveProvider;
use bookfinder::providers::archive::ArchiveProvider;
use bookfinder::providers::gutenberg::GutenbergProvider;
use bookfinder::providers::libgen::LibgenProvider;
use bookfinder::providers::BookProvider;
use bookfinder::types::{BookResult, DownloadEvent};

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

        /// Provider to search (libgen, gutenberg, archive, annas, or all)
        #[arg(short, long, default_value = "all")]
        provider: ProviderChoice,

        /// Maximum number of results per provider
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,

        /// Language filter (e.g. en, zh, ru) — used by Anna's Archive
        #[arg(long, default_value = "")]
        lang: String,
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
    Annas,
    All,
}

fn get_provider(name: &str) -> Result<Box<dyn BookProvider>> {
    match name {
        "libgen" => Ok(Box::new(LibgenProvider::new())),
        "gutenberg" => Ok(Box::new(GutenbergProvider::new())),
        "archive" => Ok(Box::new(ArchiveProvider::new())),
        "annas" | "annas-archive" => Ok(Box::new(AnnasArchiveProvider::new(""))),
        _ => bail!("Unknown provider: {}. Use libgen, gutenberg, archive, or annas.", name),
    }
}

fn get_providers(choice: &ProviderChoice, lang: &str) -> Vec<Box<dyn BookProvider>> {
    match choice {
        ProviderChoice::Libgen => vec![Box::new(LibgenProvider::new())],
        ProviderChoice::Gutenberg => vec![Box::new(GutenbergProvider::new())],
        ProviderChoice::Archive => vec![Box::new(ArchiveProvider::new())],
        ProviderChoice::Annas => vec![Box::new(AnnasArchiveProvider::new(lang))],
        ProviderChoice::All => vec![
            Box::new(LibgenProvider::new()),
            Box::new(GutenbergProvider::new()),
            Box::new(ArchiveProvider::new()),
            Box::new(AnnasArchiveProvider::new(lang)),
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

async fn do_search(query: &str, provider_choice: &ProviderChoice, limit: usize, lang: &str) -> Result<()> {
    let providers = get_providers(provider_choice, lang);
    let mut all_results: Vec<BookResult> = Vec::new();

    for p in &providers {
        println!("Searching {}...", p.name().cyan());
        match p.search(query).await {
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

async fn do_download(provider_name: &str, id: &str, output_dir: &str) -> Result<()> {
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

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let book_clone = book.clone();

    tokio::spawn(async move {
        provider.download_stream(&book_clone, tx).await;
    });

    let mut file: Option<std::fs::File> = None;
    let mut pb: Option<indicatif::ProgressBar> = None;

    while let Some(event) = rx.recv().await {
        match event? {
            DownloadEvent::FileInfo { filename, total_bytes, .. } => {
                let path = Path::new(output_dir).join(&filename);
                file = Some(std::fs::File::create(&path)?);
                if let Some(total) = total_bytes {
                    let bar = indicatif::ProgressBar::new(total);
                    bar.set_style(indicatif::ProgressStyle::with_template(
                        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})"
                    ).unwrap().progress_chars("#>-"));
                    pb = Some(bar);
                }
                println!("Downloading: {}", filename);
            }
            DownloadEvent::Data { bytes, downloaded } => {
                use std::io::Write;
                if let Some(ref mut f) = file {
                    f.write_all(&bytes)?;
                }
                if let Some(ref bar) = pb {
                    bar.set_position(downloaded);
                }
            }
            DownloadEvent::Done { filename, total_bytes } => {
                if let Some(ref bar) = pb { bar.finish_and_clear(); }
                println!(
                    "{} Downloaded {} ({:.1} KB)",
                    "Success!".green().bold(),
                    filename,
                    total_bytes as f64 / 1024.0,
                );
            }
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    tokio::runtime::Runtime::new()?.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Search {
            query,
            provider,
            limit,
            lang,
        } => do_search(query, provider, *limit, lang).await,
        Commands::Download {
            provider,
            id,
            output,
        } => do_download(provider, id, output).await,
    }
}
