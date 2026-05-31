//! Geokode CLI — index building, batch geocoding, and server.

use clap::{Parser, Subcommand};
use geokode_core::geocode::GeocoderBuilder;
use geokode_ingest::openaddresses::ingest_openaddresses;
use geokode_server::create_router;
use std::fs;

#[derive(Parser)]
#[command(name = "geokode", about = "Fast self-hosted geocoding service")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the HTTP server.
    Serve {
        /// Address data file (OpenAddresses CSV).
        #[arg(short, long)]
        data: String,
        /// Listen address.
        #[arg(short, long, default_value = "0.0.0.0:3000")]
        bind: String,
    },
    /// Forward geocode a single address.
    Forward {
        /// Address data file.
        #[arg(short, long)]
        data: String,
        /// Query string.
        query: String,
    },
    /// Reverse geocode from coordinates.
    Reverse {
        /// Address data file.
        #[arg(short, long)]
        data: String,
        /// Longitude.
        #[arg(long)]
        lon: f64,
        /// Latitude.
        #[arg(long)]
        lat: f64,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { data, bind } => {
            geokode_server::init_tracing();
            let geocoder = load_geocoder(&data);
            println!("Loaded {} records", geocoder.len());
            println!("Listening on http://{bind}");
            let app = create_router(geocoder);
            let listener = tokio::net::TcpListener::bind(&bind).await.unwrap();
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        }
        Commands::Forward { data, query } => {
            let geocoder = load_geocoder(&data);
            let results = geocoder.forward(&query);
            for r in &results {
                println!(
                    "{:.6}, {:.6} — {} (confidence: {:.2})",
                    r.lat, r.lon, r.address.full, r.confidence
                );
            }
            if results.is_empty() {
                println!("No results found.");
            }
        }
        Commands::Reverse { data, lon, lat } => {
            let geocoder = load_geocoder(&data);
            let results = geocoder.reverse(lon, lat, 5);
            for r in &results {
                println!(
                    "{:.6}, {:.6} — {} (confidence: {:.2})",
                    r.lat, r.lon, r.address.full, r.confidence
                );
            }
            if results.is_empty() {
                println!("No results found.");
            }
        }
    }
}

fn load_geocoder(path: &str) -> geokode_core::geocode::Geocoder {
    let mut builder = GeocoderBuilder::new();

    if path.ends_with(".csv") {
        let data = fs::read(path).expect("failed to read data file");
        let count =
            ingest_openaddresses(data.as_slice(), &mut builder).expect("failed to parse CSV");
        eprintln!("Ingested {count} records from {path}");
    } else if path.ends_with(".geojson") || path.ends_with(".json") {
        let data = fs::read_to_string(path).expect("failed to read data file");
        let count = geokode_ingest::geojson::ingest_geojson(&data, &mut builder)
            .expect("failed to parse GeoJSON");
        eprintln!("Ingested {count} records from {path}");
    } else {
        // Try as CSV
        let data = fs::read(path).expect("failed to read data file");
        let count =
            ingest_openaddresses(data.as_slice(), &mut builder).expect("failed to parse file");
        eprintln!("Ingested {count} records from {path}");
    }

    builder.build().expect("failed to build geocoder index")
}
