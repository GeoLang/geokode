use clap::{Parser, Subcommand};
use geokode_core::address::{GeoResult, MatchType};
use geokode_core::geocode::Geocoder;
use geokode_ingest::build::{BuildInput, build};
use geokode_server::create_router;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "geokode",
    about = "Self-hosted geocoding over OpenStreetMap data"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Build an index directory from an OSM PBF and optional address files")]
    Build {
        #[arg(long, help = "OSM PBF with the named objects and admin boundaries")]
        pbf: PathBuf,
        #[arg(
            long,
            help = "Address input: OSM PBF, OpenAddresses CSV or GeoJSON, repeatable"
        )]
        addresses: Vec<PathBuf>,
        #[arg(long, help = "Directory to write the index to")]
        out: PathBuf,
    },
    #[command(about = "Serve an index over HTTP")]
    Serve {
        #[arg(short, long)]
        index: PathBuf,
        #[arg(short, long, default_value = "0.0.0.0:3000")]
        bind: String,
    },
    #[command(about = "Forward geocode one query")]
    Forward {
        #[arg(short, long)]
        index: PathBuf,
        query: String,
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    #[command(about = "Reverse geocode one point")]
    Reverse {
        #[arg(short, long)]
        index: PathBuf,
        #[arg(long)]
        lon: f64,
        #[arg(long)]
        lat: f64,
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
}

fn open(index: &Path) -> Result<Geocoder, String> {
    Geocoder::open(index).map_err(|error| error.to_string())
}

fn print_results(results: &[GeoResult]) {
    if results.is_empty() {
        println!("No results found.");
    }
    for result in results {
        let tag = match result.match_type {
            MatchType::Exact => "",
            MatchType::Prefix => ", prefix",
            MatchType::Fuzzy => ", fuzzy",
        };
        println!(
            "{:.6}, {:.6}  {} [{:?}] (confidence: {:.2}{tag})",
            result.lat, result.lon, result.display_name, result.kind, result.confidence
        );
    }
}

async fn run(command: Commands) -> Result<(), String> {
    match command {
        Commands::Build {
            pbf,
            addresses,
            out,
        } => {
            let summary = build(&BuildInput {
                pbf: &pbf,
                addresses: &addresses,
                out: &out,
            })
            .map_err(|error| error.to_string())?;
            println!("{} records, {} index keys", summary.records, summary.keys);
            for (kind, count) in summary.by_kind {
                println!("  {kind:?}: {count}");
            }
        }
        Commands::Serve { index, bind } => {
            geokode_server::init_tracing();
            let geocoder = open(&index)?;
            println!("Loaded {} records from {}", geocoder.len(), index.display());
            println!("Listening on http://{bind}");
            let listener = tokio::net::TcpListener::bind(&bind)
                .await
                .map_err(|error| format!("cannot listen on {bind}: {error}"))?;
            axum::serve(listener, create_router(geocoder).into_make_service())
                .await
                .map_err(|error| error.to_string())?;
        }
        Commands::Forward {
            index,
            query,
            limit,
        } => print_results(&open(&index)?.forward(&query, limit, None)),
        Commands::Reverse {
            index,
            lon,
            lat,
            limit,
        } => print_results(&open(&index)?.reverse(lon, lat, limit)),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse().command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("geokode: {message}");
            ExitCode::FAILURE
        }
    }
}
