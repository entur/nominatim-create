mod common;
mod config;
mod source;
mod target;

use clap::{Parser, Subcommand};
use common::input::{cleanup_input, resolve_input};
use common::norwegian_counties;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "nominatim-converter", about = "Convert geographic data to Nominatim NDJSON")]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Convert StopPlace NeTEx XML
    Stopplace(ConvertArgs),
    /// Convert Matrikkel CSV data (Kartverket)
    Matrikkel(MatrikkelArgs),
    /// Convert OSM PBF data
    Osm(ConvertArgs),
    /// Convert Stedsnavn GML data (Kartverket)
    Stedsnavn(StedsnavnArgs),
    /// Convert POI NeTEx XML data
    Poi(ConvertArgs),
    /// Convert Swedish belägenhetsadresser (Lantmäteriet)
    Belagenhet(BelagenhetArgs),
}

fn geonorge_matrikkel_url(region: &str) -> String {
    format!("https://nedlasting.geonorge.no/geonorge/Basisdata/MatrikkelenAdresse/CSV/Basisdata_{region}_25833_MatrikkelenAdresse_CSV.zip")
}

fn geonorge_stedsnavn_url(region: &str) -> String {
    format!("https://nedlasting.geonorge.no/geonorge/Basisdata/Stedsnavn/GML/Basisdata_{region}_25833_Stedsnavn_GML.zip")
}

#[derive(Parser)]
struct ConvertArgs {
    /// Input file or URL (http/https). ZIP archives are extracted automatically.
    #[arg(short = 'i')]
    input: PathBuf,
    /// Output file
    #[arg(short = 'o')]
    output: PathBuf,
    /// Configuration file (defaults to converter.json)
    #[arg(short = 'c')]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short = 'a', default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short = 'f', default_value_t = false)]
    force: bool,
}

#[derive(Parser)]
struct BelagenhetArgs {
    /// Input .gpkg file (required when -m is not used)
    #[arg(short = 'i')]
    input: Option<PathBuf>,
    /// Output file
    #[arg(short = 'o', required_unless_present = "list")]
    output: Option<PathBuf>,
    /// Configuration file (defaults to converter.json)
    #[arg(short = 'c')]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short = 'a', default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short = 'f', default_value_t = false)]
    force: bool,
    /// Municipality code(s) to download from Lantmäteriet. Use "all" for entire Sweden,
    /// a 2-digit county code (e.g. 01) for all municipalities in a county,
    /// or specific 4-digit codes (e.g. 0180). Requires LANTMATERIET_USER/PASS env vars.
    #[arg(short = 'm', long = "municipality", num_args = 1.., conflicts_with = "input")]
    municipality: Option<Vec<String>>,
    /// List available municipalities for download
    #[arg(short = 'l', long = "list", default_value_t = false)]
    list: bool,
}

#[derive(Parser)]
struct StedsnavnArgs {
    /// Input file or URL (use -m to download from Geonorge instead)
    #[arg(short = 'i', conflicts_with = "region")]
    input: Option<PathBuf>,
    /// Output file
    #[arg(short = 'o', required_unless_present = "list")]
    output: Option<PathBuf>,
    /// Configuration file (defaults to converter.json)
    #[arg(short = 'c')]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short = 'a', default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short = 'f', default_value_t = false)]
    force: bool,
    /// Download from Geonorge. Use a county code (e.g. 03 for Oslo), county name,
    /// or "0000" / "all" for all of Norway.
    #[arg(short = 'r', long = "region")]
    region: Option<String>,
    /// List available regions for download
    #[arg(short = 'l', long = "list", default_value_t = false)]
    list: bool,
}

#[derive(Parser)]
struct MatrikkelArgs {
    /// Input CSV file or URL (use -m to download from Geonorge instead)
    #[arg(short = 'i', conflicts_with = "region")]
    input: Option<PathBuf>,
    /// Output file
    #[arg(short = 'o', required_unless_present = "list")]
    output: Option<PathBuf>,
    /// Configuration file (defaults to converter.json)
    #[arg(short = 'c')]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short = 'a', default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short = 'f', default_value_t = false)]
    force: bool,
    /// Download from Geonorge. Use a county code (e.g. 03 for Oslo), county name,
    /// or "0000" / "all" for all of Norway.
    #[arg(short = 'r', long = "region")]
    region: Option<String>,
    /// List available regions for download
    #[arg(short = 'l', long = "list", default_value_t = false)]
    list: bool,
    /// Stedsnavn GML file or URL for county data (auto-downloaded when using -r)
    #[arg(short = 'g')]
    stedsnavn_gml: Option<PathBuf>,
    /// Skip county population
    #[arg(long = "no-county", default_value_t = false)]
    no_county: bool,
}

fn main() {
    // Suppress "Cannot find proj.db" warnings from bundled PROJ.
    // We use a pipeline string that doesn't need the database.
    if std::env::var_os("PROJ_DATA").is_none() {
        // SAFETY: called at the start of main before any other threads are spawned.
        unsafe { std::env::set_var("PROJ_DATA", "/dev/null") };
    }

    // Load .env file for credentials (Lantmäteriet etc.) -- once, before any subcommand runs.
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    let result = match cli.action {
        Action::Stopplace(args) => run_conversion("StopPlace", args, Some("*.xml"), |cfg, input, output, append| {
            source::stopplace::convert(cfg, input, output, append)
        }),
        Action::Matrikkel(args) => {
            if args.list {
                norwegian_counties::list_regions();
                return;
            }
            let input_path = match (&args.input, &args.region) {
                (Some(path), _) => path.clone(),
                (None, Some(region)) => {
                    let slug = resolve_geonorge_region(region);
                    PathBuf::from(geonorge_matrikkel_url(&slug))
                }
                (None, None) => {
                    eprintln!("Error: matrikkel requires -i <file> or -r <region> (e.g. -r 03 for Oslo, -r all for Norway)");
                    std::process::exit(1);
                }
            };

            // Auto-download stedsnavn GML for county data when using -m
            let gml_source = if args.no_county {
                None
            } else if let Some(gml) = args.stedsnavn_gml {
                Some(gml)
            } else if let Some(region) = &args.region {
                let slug = resolve_geonorge_region(region);
                Some(PathBuf::from(geonorge_stedsnavn_url(&slug)))
            } else {
                eprintln!("Error: matrikkel requires -g <stedsnavn.gml> for county data, or --no-county to skip it.");
                std::process::exit(1);
            };

            let gml_resolved = gml_source.as_ref().map(|g| resolve_input(g, Some("*.gml")));
            let gml_result = match gml_resolved {
                Some(Ok((path, is_temp))) => Some((path, is_temp)),
                Some(Err(e)) => {
                    eprintln!("Error resolving GML input: {e}");
                    std::process::exit(1);
                }
                None => None,
            };

            let convert_args = ConvertArgs {
                input: input_path,
                output: args.output.unwrap(),
                config: args.config,
                append: args.append,
                force: args.force,
            };

            let gml_path = gml_result.as_ref().map(|(p, _)| p.as_path());
            let result = run_conversion("Matrikkel", convert_args, Some("*.csv"), |cfg, input, output, append| {
                source::matrikkel::convert(cfg, input, output, append, gml_path)
            });

            if let Some((path, is_temp)) = gml_result {
                cleanup_input(&path, is_temp);
            }

            result
        }
        Action::Osm(args) => run_conversion("OSM PBF", args, None, |cfg, input, output, append| {
            source::osm::convert(cfg, input, output, append)
        }),
        Action::Stedsnavn(args) => {
            if args.list {
                norwegian_counties::list_regions();
                return;
            }
            let input_path = match (&args.input, &args.region) {
                (Some(path), _) => path.clone(),
                (None, Some(region)) => {
                    let slug = resolve_geonorge_region(region);
                    PathBuf::from(geonorge_stedsnavn_url(&slug))
                }
                (None, None) => {
                    eprintln!("Error: stedsnavn requires -i <file> or -r <region> (e.g. -r 03 for Oslo, -r all for Norway)");
                    std::process::exit(1);
                }
            };
            let convert_args = ConvertArgs {
                input: input_path,
                output: args.output.unwrap(),
                config: args.config,
                append: args.append,
                force: args.force,
            };
            run_conversion("Stedsnavn", convert_args, Some("*.gml"), |cfg, input, output, append| {
                source::stedsnavn::convert(cfg, input, output, append)
            })
        }
        Action::Poi(args) => run_conversion("POI", args, None, |cfg, input, output, append| {
            source::poi::convert(cfg, input, output, append)
        }),
        Action::Belagenhet(args) => {
            if args.list {
                list_swedish_municipalities();
                return;
            }
            if let Some(ref municipalities) = args.municipality {
                let codes = resolve_municipality_codes(municipalities);
                run_belagenhet_download(&args, &codes)
            } else {
                let input = args.input.as_ref().unwrap_or_else(|| {
                    eprintln!("Error: belagenhet requires either -i <file.gpkg> or -m <municipality_code>");
                    std::process::exit(1);
                });
                let convert_args = ConvertArgs {
                    input: input.clone(),
                    output: args.output.unwrap(),
                    config: args.config,
                    append: args.append,
                    force: args.force,
                };
                run_conversion("Belägenhetsadress", convert_args, Some("*.gpkg"), |cfg, input, output, append| {
                    source::belagenhet::convert(cfg, input, output, append)
                })
            }
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run_conversion<F>(
    name: &str,
    args: ConvertArgs,
    extract_glob: Option<&str>,
    convert_fn: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&config::Config, &std::path::Path, &std::path::Path, bool) -> Result<(), Box<dyn std::error::Error>>,
{
    let cfg = config::Config::load(args.config.as_deref())?;

    let output = &args.output;
    if output.exists() {
        if !args.force && !args.append {
            return Err(format!(
                "Output file '{}' already exists. Use -f to overwrite or -a to append.",
                output.display()
            ).into());
        }
        if args.force {
            eprintln!("Overwriting existing file: {}", output.display());
            std::fs::remove_file(output)?;
        } else if args.append {
            eprintln!("Appending to existing file: {}", output.display());
        }
    }

    let (input, is_temp) = resolve_input(&args.input, extract_glob)?;

    eprintln!("Starting {name} conversion...");
    let start = Instant::now();
    let result = convert_fn(&cfg, &input, output, args.append);

    cleanup_input(&input, is_temp);

    result?;

    let duration = start.elapsed().as_secs_f64();
    let size_mb = std::fs::metadata(output).map(|m| m.len() as f64 / (1024.0 * 1024.0)).unwrap_or(0.0);
    let action = if args.append { "Appended to" } else { "Output written to" };
    eprintln!("{name} conversion completed in {duration:.2} seconds. {action} {}, size: {size_mb:.2} MB.", output.display());
    Ok(())
}

fn run_belagenhet_download(
    args: &BelagenhetArgs,
    municipalities: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = config::Config::load(args.config.as_deref())?;
    let output = args.output.as_ref().expect("output required for download");

    if output.exists() {
        if !args.force && !args.append {
            return Err(format!(
                "Output file '{}' already exists. Use -f to overwrite or -a to append.",
                output.display()
            ).into());
        }
        if args.force {
            eprintln!("Overwriting existing file: {}", output.display());
            std::fs::remove_file(output)?;
        }
    }

    let start = Instant::now();
    let mut is_first = !args.append;

    for (i, kommun_id) in municipalities.iter().enumerate() {
        eprintln!("Processing municipality {kommun_id} ({}/{})...", i + 1, municipalities.len());

        let gpkg_path = source::belagenhet::download::download_municipality(kommun_id)?;
        let appending = !is_first;

        source::belagenhet::convert(&cfg, &gpkg_path, output, appending)?;

        std::fs::remove_file(&gpkg_path).ok();
        is_first = false;
    }

    let duration = start.elapsed().as_secs_f64();
    let size_mb = std::fs::metadata(output).map(|m| m.len() as f64 / (1024.0 * 1024.0)).unwrap_or(0.0);
    eprintln!(
        "Belägenhetsadress conversion completed in {duration:.2} seconds. {} municipalities processed. Output: {}, size: {size_mb:.2} MB.",
        municipalities.len(),
        output.display()
    );
    Ok(())
}

fn resolve_geonorge_region(region: &str) -> String {
    match norwegian_counties::resolve_geonorge_region(region) {
        Ok(slug) => slug,
        Err(msg) => {
            eprintln!("Error: {msg}");
            norwegian_counties::list_regions();
            std::process::exit(1);
        }
    }
}

fn list_swedish_municipalities() {
    use source::belagenhet::municipalities::MUNICIPALITIES;
    eprintln!("Available municipalities for Lantmäteriet download:");
    eprintln!("  all         All {} municipalities", MUNICIPALITIES.len());
    eprintln!("  XX          All municipalities in county XX (2-digit county code)");
    eprintln!();
    for (code, name) in MUNICIPALITIES {
        eprintln!("  {code}  {name}");
    }
}

/// Expand municipality arguments: "all" becomes all 290 codes, county prefixes (2-digit)
/// expand to all municipalities in that county, otherwise codes are passed through as-is.
fn resolve_municipality_codes(args: &[String]) -> Vec<String> {
    use source::belagenhet::municipalities::MUNICIPALITIES;

    let mut codes = Vec::new();
    for arg in args {
        let lower = arg.to_lowercase();
        if lower == "all" || lower == "00" {
            eprintln!("Expanding 'all' to all {} municipalities", MUNICIPALITIES.len());
            return MUNICIPALITIES.iter().map(|(c, _)| c.to_string()).collect();
        } else if arg.len() == 2 && arg.chars().all(|c| c.is_ascii_digit()) {
            // County prefix: expand to all municipalities in that län
            let matching: Vec<String> = MUNICIPALITIES.iter()
                .filter(|(c, _)| c.starts_with(arg.as_str()))
                .map(|(c, _)| c.to_string())
                .collect();
            if matching.is_empty() {
                eprintln!("Warning: no municipalities found for county code {arg}");
            } else {
                eprintln!("Expanding county {arg} to {} municipalities", matching.len());
                codes.extend(matching);
            }
        } else {
            codes.push(arg.clone());
        }
    }
    codes
}
