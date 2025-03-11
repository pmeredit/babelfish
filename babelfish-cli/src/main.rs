use ast::definitions::Pipeline;
use babelfish::*;
use clap::Parser;
use schema::Erd;

#[derive(Debug)]
pub enum CliError {
    Io(std::io::Error),
    Bson(bson::de::Error),
    Json(serde_json::Error),
    Babelfish(babelfish::assemble_rewrite::Error),
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Io(e)
    }
}

impl From<bson::de::Error> for CliError {
    fn from(e: bson::de::Error) -> Self {
        CliError::Bson(e)
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        CliError::Json(e.into())
    }
}

impl From<babelfish::assemble_rewrite::Error> for CliError {
    fn from(e: babelfish::assemble_rewrite::Error) -> Self {
        CliError::Babelfish(e)
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about=None)]
struct Cli {
    #[arg(short, long, help = "pipeline bson file")]
    pipeline_file: Option<String>,
    #[arg(short, long, help = "verbose mode")]
    erd_file: Option<String>,
}

fn main() -> Result<(), CliError> {
    let args = Cli::parse();

    if let Some(erd_file) = &args.erd_file {
        let erd = std::fs::read_to_string(erd_file)?;
        let erd: Erd = serde_json::from_str(&erd)?;
        println!("{:?}", erd);
    }
    if let Some(pipeline_file) = &args.pipeline_file {
        let pipeline = std::fs::read_to_string(pipeline_file)?;
        let pipeline: Pipeline = serde_json::from_str(&pipeline)?;
        let pipeline = assemble_rewrite::rewrite_pipeline(pipeline)?;
        let pipeline_json = serde_json::to_string_pretty(&pipeline)?;
        println!("{}", pipeline_json);
    }
    Ok(())
}
