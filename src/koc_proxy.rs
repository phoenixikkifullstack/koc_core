use clap::{Parser, Subcommand, ValueEnum};
use koc_core::proxy::{RelayConfig, run_relay};
use koc_core::proxy_capture::{
    CaptureRecord, CommandCatalog, DecodedEvent, Direction, FrameOpcode, Inspector, ObservedFrame,
    format_pretty_event, matches_command, open_secure_writer, read_records, rewrite_json_pretty,
    write_json_line, write_json_pretty,
};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, watch};
use url::Url;

#[derive(Parser, Debug)]
#[command(
    name = "koc_proxy",
    version,
    about = "Realtime KOC WebSocket relay and protocol inspector"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Relay WebSocket traffic and decode game messages in realtime
    Relay(RelayArgs),
    /// Decode capture records from stdin as a realtime stream
    Inspect(InspectArgs),
    /// Decode a saved JSONL capture
    Decode(DecodeArgs),
    /// Build a command and payload-shape catalog from a capture
    Catalog(CatalogArgs),
}

#[derive(clap::Args, Debug)]
struct RelayArgs {
    /// Loopback address on which the relay accepts WebSocket clients
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,

    /// Upstream WebSocket origin; the inbound path and query are forwarded unchanged
    #[arg(long, default_value = "wss://xxz-xyzw.hortorgames.com")]
    upstream: Url,

    /// Disable realtime decoded console output while still allowing raw record/catalog output
    #[arg(long, default_value_t = false, conflicts_with = "decode")]
    no_decode: bool,

    /// Explicitly enable realtime decoded console output (enabled by default)
    #[arg(long, default_value_t = false)]
    decode: bool,

    /// Optional raw JSONL capture output; decode it later to view cmd/body fields
    #[arg(long, value_name = "FILE")]
    record: Option<PathBuf>,

    /// Optional command catalog JSON snapshot, updated while the relay runs
    #[arg(long, value_name = "FILE")]
    catalog: Option<PathBuf>,

    #[command(flatten)]
    display: DisplayArgs,

    /// Capacity of the non-blocking capture analysis channel
    #[arg(long, default_value_t = 256)]
    channel_capacity: usize,

    /// Maximum number of simultaneous relayed connections
    #[arg(long, default_value_t = 16)]
    max_connections: usize,

    /// Maximum total payload bytes retained by the capture queue
    #[arg(long, default_value_t = 32)]
    max_queued_capture_mb: usize,
}

#[derive(clap::Args, Debug)]
struct InspectArgs {
    /// Read JSONL capture records continuously from stdin
    #[arg(long, default_value_t = false)]
    stream: bool,

    #[command(flatten)]
    display: DisplayArgs,
}

#[derive(clap::Args, Debug)]
struct DecodeArgs {
    /// JSONL capture file, or '-' for stdin
    #[arg(long, value_name = "FILE")]
    input: PathBuf,

    #[command(flatten)]
    display: DisplayArgs,
}

#[derive(clap::Args, Debug)]
struct CatalogArgs {
    /// JSONL capture file, or '-' for stdin
    #[arg(long, value_name = "FILE")]
    input: PathBuf,

    /// Catalog output file; omit or use '-' for stdout
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Do not redact sensitive values before shape inference
    #[arg(long, default_value_t = false)]
    show_sensitive: bool,
}

#[derive(clap::Args, Debug, Clone)]
struct DisplayArgs {
    /// Decoded event output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
    format: OutputFormat,

    /// Command filter with '*' wildcard support
    #[arg(long)]
    cmd: Option<String>,

    /// Direction filter
    #[arg(long, value_enum)]
    direction: Option<DirectionArg>,

    /// Include text and WebSocket control frames in decoded output
    #[arg(long, default_value_t = false)]
    include_control: bool,

    /// Print sensitive decoded values; handshake tokens are never captured
    #[arg(long, default_value_t = false)]
    show_sensitive: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Pretty,
    Json,
    Silent,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DirectionArg {
    ClientToServer,
    ServerToClient,
}

impl From<DirectionArg> for Direction {
    fn from(value: DirectionArg) -> Self {
        match value {
            DirectionArg::ClientToServer => Self::ClientToServer,
            DirectionArg::ServerToClient => Self::ServerToClient,
        }
    }
}

struct Analyzer {
    inspector: Inspector,
    display: DisplayArgs,
    record_writer: Option<BufWriter<File>>,
    catalog_writer: Option<BufWriter<File>>,
    catalog: Option<CommandCatalog>,
    realtime_output: bool,
}

impl Analyzer {
    fn new(
        display: DisplayArgs,
        record_path: Option<&Path>,
        catalog_path: Option<PathBuf>,
        realtime_output: bool,
    ) -> Result<Self, String> {
        let record_writer = record_path.map(open_secure_writer).transpose()?;
        let catalog_writer = catalog_path
            .as_deref()
            .map(open_secure_writer)
            .transpose()?;
        Ok(Self {
            inspector: Inspector::new(!display.show_sensitive),
            display,
            record_writer,
            catalog_writer,
            catalog: catalog_path.as_ref().map(|_| CommandCatalog::new()),
            realtime_output,
        })
    }

    fn process(&mut self, record: CaptureRecord) -> Result<(), String> {
        if let Some(writer) = self.record_writer.as_mut() {
            write_json_line(writer, &record)?;
            writer.flush().map_err(|e| e.to_string())?;
        }
        let event = self.inspector.inspect(&record);
        if let Some(catalog) = self.catalog.as_mut() {
            catalog.observe(&event);
        }
        if let (Some(writer), Some(catalog)) = (self.catalog_writer.as_mut(), self.catalog.as_ref())
        {
            rewrite_json_pretty(writer, catalog)?;
        }
        if self.realtime_output && self.should_display(&event) {
            print_event(self.display.format, &event)?;
        }
        Ok(())
    }

    fn should_display(&self, event: &DecodedEvent) -> bool {
        if !self.display.include_control && event.opcode != FrameOpcode::Binary {
            return false;
        }
        if let Some(direction) = self.display.direction {
            if event.direction != Direction::from(direction) {
                return false;
            }
        }
        matches_command(self.display.cmd.as_deref(), event)
    }

    fn finish(mut self) -> Result<(), String> {
        if let Some(writer) = self.record_writer.as_mut() {
            writer.flush().map_err(|e| e.to_string())?;
        }
        if let (Some(writer), Some(catalog)) = (self.catalog_writer.as_mut(), self.catalog) {
            rewrite_json_pretty(writer, &catalog)?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Err(error) = run(args.command).await {
        eprintln!("Error: {}", error);
        std::process::exit(1);
    }
}

async fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Relay(args) => run_relay_command(args).await,
        Command::Inspect(args) => {
            if !args.stream {
                return Err(
                    "inspect currently requires --stream and reads JSONL from stdin".to_string(),
                );
            }
            let stdin = std::io::stdin();
            decode_reader(stdin.lock(), args.display)
        }
        Command::Decode(args) => {
            with_input(&args.input, |reader| decode_reader(reader, args.display))
        }
        Command::Catalog(args) => with_input(&args.input, |reader| {
            catalog_reader(reader, args.output.as_deref(), args.show_sensitive)
        }),
    }
}

async fn run_relay_command(args: RelayArgs) -> Result<(), String> {
    if !(1..=4096).contains(&args.channel_capacity) {
        return Err("channel_capacity must be between 1 and 4096".to_string());
    }
    if !(1..=256).contains(&args.max_connections) {
        return Err("max_connections must be between 1 and 256".to_string());
    }
    if !(1..=256).contains(&args.max_queued_capture_mb) {
        return Err("max_queued_capture_mb must be between 1 and 256".to_string());
    }
    if let (Some(record), Some(catalog)) = (&args.record, &args.catalog) {
        let record = std::path::absolute(record).map_err(|e| e.to_string())?;
        let catalog = std::path::absolute(catalog).map_err(|e| e.to_string())?;
        if record == catalog {
            return Err("record and catalog outputs must use different files".to_string());
        }
    }
    for path in args.record.iter().chain(args.catalog.iter()) {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("refusing to overwrite symlink: {}", path.display()));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!("output path is not a file: {}", path.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("failed to inspect {}: {}", path.display(), error));
            }
        }
    }
    let analysis_enabled =
        args.decode || !args.no_decode || args.record.is_some() || args.catalog.is_some();
    let relay_config = RelayConfig {
        listen: args.listen,
        upstream_origin: args.upstream,
        max_connections: args.max_connections,
        max_capture_queue_bytes: args.max_queued_capture_mb * 1024 * 1024,
    };
    if !analysis_enabled {
        return run_unobserved_relay(relay_config).await;
    }

    let analyzer = Analyzer::new(
        args.display,
        args.record.as_deref(),
        args.catalog,
        args.decode || !args.no_decode,
    )?;
    let (capture_tx, mut capture_rx) = mpsc::channel::<ObservedFrame>(args.channel_capacity);
    let mut analyzer_handle = tokio::task::spawn_blocking(move || {
        let mut analyzer = analyzer;
        while let Some(observed) = capture_rx.blocking_recv() {
            analyzer.process(observed.into_record())?;
        }
        analyzer.finish()
    });
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut relay_handle = tokio::spawn(run_relay(relay_config, Some(capture_tx), shutdown_rx));

    tokio::select! {
        signal = shutdown_signal() => {
            signal?;
            let _ = shutdown_tx.send(true);
            let relay_result = join_relay(relay_handle.await);
            let analyzer_result = join_analyzer(analyzer_handle.await);
            let stats = relay_result?;
            print_relay_stats(stats);
            analyzer_result?;
            Ok(())
        }
        relay_result = &mut relay_handle => {
            let relay_result = join_relay(relay_result);
            let analyzer_result = join_analyzer(analyzer_handle.await);
            let stats = relay_result?;
            print_relay_stats(stats);
            analyzer_result?;
            Ok(())
        }
        analyzer_result = &mut analyzer_handle => {
            let analyzer_result = join_analyzer(analyzer_result);
            let _ = shutdown_tx.send(true);
            let relay_result = join_relay(relay_handle.await);
            let stats = relay_result?;
            print_relay_stats(stats);
            analyzer_result?;
            Ok(())
        }
    }
}

fn decode_reader(reader: impl BufRead, display: DisplayArgs) -> Result<(), String> {
    let mut analyzer = Analyzer::new(display, None, None, true)?;
    read_records(reader, |record| analyzer.process(record))?;
    analyzer.finish()
}

fn catalog_reader(
    reader: impl BufRead,
    output: Option<&Path>,
    show_sensitive: bool,
) -> Result<(), String> {
    let mut inspector = Inspector::new(!show_sensitive);
    let mut catalog = CommandCatalog::new();
    read_records(reader, |record| {
        let event = inspector.inspect(&record);
        catalog.observe(&event);
        Ok(())
    })?;
    if let Some(path) = output.filter(|path| *path != Path::new("-")) {
        write_json_pretty(path, &catalog)
    } else {
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        serde_json::to_writer_pretty(&mut output, &catalog).map_err(|e| e.to_string())?;
        writeln!(output).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn with_input<T>(
    path: &Path,
    callback: impl FnOnce(Box<dyn BufRead>) -> Result<T, String>,
) -> Result<T, String> {
    if path == Path::new("-") {
        callback(Box::new(BufReader::new(std::io::stdin())))
    } else {
        let file = File::open(path)
            .map_err(|error| format!("failed to open {}: {}", path.display(), error))?;
        callback(Box::new(BufReader::new(file)))
    }
}

fn print_event(format: OutputFormat, event: &DecodedEvent) -> Result<(), String> {
    match format {
        OutputFormat::Pretty => {
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            writeln!(output, "{}\n", format_pretty_event(event)).map_err(|e| e.to_string())
        }
        OutputFormat::Json => {
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            serde_json::to_writer(&mut output, event).map_err(|e| e.to_string())?;
            writeln!(output).map_err(|e| e.to_string())
        }
        OutputFormat::Silent => Ok(()),
    }
}

fn join_relay(
    result: Result<Result<koc_core::proxy::RelayStats, String>, tokio::task::JoinError>,
) -> Result<koc_core::proxy::RelayStats, String> {
    result.map_err(|e| format!("relay task failed: {}", e))?
}

fn join_analyzer(result: Result<Result<(), String>, tokio::task::JoinError>) -> Result<(), String> {
    result.map_err(|e| format!("analyzer task failed: {}", e))?
}

fn print_relay_stats(stats: koc_core::proxy::RelayStats) {
    eprintln!(
        "koc_proxy stopped: connections={}, dropped_queue_full={}, dropped_analyzer_closed={}",
        stats.accepted_connections, stats.dropped_queue_full, stats.dropped_analyzer_closed
    );
}

async fn run_unobserved_relay(config: RelayConfig) -> Result<(), String> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut relay_handle = tokio::spawn(run_relay(config, None, shutdown_rx));
    let stats = tokio::select! {
        signal = shutdown_signal() => {
            signal?;
            let _ = shutdown_tx.send(true);
            join_relay(relay_handle.await)?
        }
        result = &mut relay_handle => join_relay(result)?,
    };
    print_relay_stats(stats);
    Ok(())
}

async fn shutdown_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|e| format!("failed to listen for SIGTERM: {}", e))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.map_err(|e| format!("failed to listen for Ctrl+C: {}", e))?;
            }
            _ = terminate.recv() => {}
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|e| format!("failed to listen for Ctrl+C: {}", e))
    }
}
