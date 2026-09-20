use clap::Parser;

mod models;

/// A Docker sbx-style TUI for microsandbox.
#[derive(Parser, Debug)]
#[command(name = "msb-tui", version, about)]
struct Cli {
    /// Open the create-sandbox form directly.
    #[arg(short, long)]
    create: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Verify msb is installed and on PATH
    which::which("msb").map_err(|_| {
        anyhow::anyhow!(
            "msb CLI not found on PATH. Install it with:\n  \
             curl -fsSL https://install.microsandbox.dev | sh"
        )
    })?;

    // TODO: bootstrap terminal, init app state, run event loop
    eprintln!("microsandbox-tui — not yet implemented");
    eprintln!(
        "Run `msb-tui{}` to see the design.",
        if cli.create { " --create" } else { "" }
    );
    Ok(())
}
