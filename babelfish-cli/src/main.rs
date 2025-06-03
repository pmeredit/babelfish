use ast::definitions::Pipeline;
use babelfish::*;
use clap::Parser;
use schema::Erd;

#[derive(Debug)]
pub enum CliError {
    Io(std::io::Error),
    Bson(bson::de::Error),
    Json(serde_json::Error),
    Assemble(babelfish::assemble_rewrite::Error),
    AugmentedProject(babelfish::augmented_project_rewrite::Error),
    Conjure(babelfish::conjure_rewrite::Error),
    Join(babelfish::join_rewrite::Error),
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
        CliError::Assemble(e)
    }
}

impl From<babelfish::join_rewrite::Error> for CliError {
    fn from(e: babelfish::join_rewrite::Error) -> Self {
        CliError::Join(e)
    }
}

impl From<babelfish::augmented_project_rewrite::Error> for CliError {
    fn from(e: babelfish::augmented_project_rewrite::Error) -> Self {
        CliError::AugmentedProject(e)
    }
}

impl From<babelfish::conjure_rewrite::Error> for CliError {
    fn from(e: babelfish::conjure_rewrite::Error) -> Self {
        CliError::Conjure(e)
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about=None)]
struct Cli {
    #[arg(short, long, help = "pipeline bson file")]
    pipeline_file: Option<String>,
    #[arg(short, long, help = "erd file")]
    erd_file: Option<String>,
    #[arg(short, long, help = "match move")]
    match_move: Option<String>,
    #[arg(short, long, help = "new erd file")]
    nerd_file: Option<String>,
}

fn main() {
    if let Err(e) = run() {
        match e {
            CliError::Io(e) => eprintln!("IO error: {}", e),
            CliError::Bson(e) => eprintln!("Bson error: {}", e),
            CliError::Json(e) => eprintln!("Json error: {}", e),
            CliError::Assemble(e) => println!("Assemble error: {}", e),
            CliError::Join(e) => println!("Join error: {}", e),
            CliError::AugmentedProject(e) => println!("AugmentedProject error: {}", e),
            CliError::Conjure(e) => println!("Conjure error: {}", e),
        }
    }
}

fn run() -> Result<(), CliError> {
    let args = Cli::parse();

    if let Some(erd_file) = &args.erd_file {
        let erd = std::fs::read_to_string(erd_file)?;
        let erd: Erd = serde_json::from_str(&erd)?;
        println!("{:?}", erd);
    } else if let Some(pipeline_file) = &args.pipeline_file {
        let pipeline = std::fs::read_to_string(pipeline_file)?;
        let pipeline: Pipeline = serde_json::from_str(&pipeline)?;
        let pipeline = augmented_project_rewrite::rewrite_pipeline(pipeline)?;
        let pipeline = conjure_rewrite::rewrite_pipeline(pipeline)?;
        let pipeline = assemble_rewrite::rewrite_pipeline(pipeline)?;
        let pipeline = join_rewrite::rewrite_pipeline(pipeline)?;
        let pipeline = match_movement_rewrite::rewrite_match_move(pipeline);
        let pipeline_json = serde_json::to_string_pretty(&pipeline)?;
        println!("{}", pipeline_json);
    } else if let Some(match_move) = &args.match_move {
        let match_move = std::fs::read_to_string(match_move)?;
        let match_move: Pipeline = serde_json::from_str(&match_move)?;
        let match_move = match_movement_rewrite::rewrite_match_move(match_move);
        let match_move_json = serde_json::to_string_pretty(&match_move)?;
        println!("{}", match_move_json);
    } else if let Some(nerd_file) = &args.nerd_file {
        let nerd = std::fs::read_to_string(nerd_file)?;
        let nerd: babelfish::erd::Erd = serde_json::from_str(&nerd)?;
        println!("{:?}", nerd);
    }
    Ok(())
}
