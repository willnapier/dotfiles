use clap::Parser;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

#[derive(Parser)]
#[command(name = "clinical-product", about = "Clinical session note generator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser)]
enum Command {
    /// Generate a session note from an observation
    Note {
        /// The clinical observation (2-3 sentences)
        observation: String,

        /// Therapeutic modality for terminology (e.g. "ACT/CBS", "psychodynamic", "integrative CBT")
        #[arg(short, long, default_value = "ACT/CBS")]
        modality: String,

        /// Ollama API endpoint URL
        #[arg(short, long, default_value = "http://localhost:11434")]
        endpoint: String,

        /// Model name
        #[arg(long, default_value = "gemma4:26b")]
        model: String,

        /// Disable streaming (wait for full response)
        #[arg(long)]
        no_stream: bool,
    },

    /// Raw completion: reads a full prompt from stdin, streams completion to stdout.
    /// Used by `clinical note` as CLINICAL_LLM_CMD to integrate the voice model.
    Raw {
        /// Ollama API endpoint URL
        #[arg(short, long, default_value = "http://localhost:11434")]
        endpoint: String,

        /// Model name
        #[arg(long, default_value = "gemma4:26b")]
        model: String,

        /// Disable streaming (wait for full response before printing)
        #[arg(long)]
        no_stream: bool,
    },
}

#[derive(Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    system: String,
    stream: bool,
}

#[derive(Deserialize)]
struct StreamChunk {
    response: Option<String>,
    done: Option<bool>,
    total_duration: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

fn build_system_prompt(modality: &str) -> String {
    format!(
        "You are a clinical psychologist's session note writer. \
         Produce a session note in the practitioner's established style. \
         Frame clinical reasoning using explicit {} process terminology — \
         name the relevant therapeutic processes where they apply to the session material. \
         Integrate these naturally into the prose rather than listing them. \
         Structure: Risk assessment, narrative body, Formulation.",
        modality
    )
}

async fn raw_completion(
    prompt: String,
    endpoint: String,
    model: String,
    no_stream: bool,
) -> anyhow::Result<()> {
    // For raw mode, we send the entire stdin content as the prompt with
    // an empty system message — the caller (e.g. `clinical note`) has
    // already built the full context and instruction.
    let request = GenerateRequest {
        model,
        prompt,
        system: String::new(),
        stream: !no_stream,
    };

    let client = Client::new();
    let url = format!("{}/api/generate", endpoint);
    let start = std::time::Instant::now();

    if no_stream {
        let resp: serde_json::Value = client.post(&url).json(&request).send().await?.json().await?;
        let text = resp["response"].as_str().unwrap_or("");
        print!("{}", text);
        eprintln!("\n---\nGenerated in {:.1}s", start.elapsed().as_secs_f64());
    } else {
        let resp = client.post(&url).json(&request).send().await?;
        let mut stream = resp.bytes_stream();
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let stderr = io::stderr();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            if let Ok(parsed) = serde_json::from_slice::<StreamChunk>(&bytes) {
                if let Some(text) = &parsed.response {
                    write!(out, "{}", text)?;
                    out.flush()?;
                }
                if parsed.done == Some(true) {
                    if let (Some(ec), Some(ed)) = (parsed.eval_count, parsed.eval_duration) {
                        let tps = if ed > 0 { ec as f64 / (ed as f64 / 1e9) } else { 0.0 };
                        let mut err = stderr.lock();
                        writeln!(err)?;
                        writeln!(
                            err,
                            "---\nGenerated {} tokens in {:.1}s ({:.0} tok/s)",
                            ec,
                            start.elapsed().as_secs_f64(),
                            tps
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Raw {
            endpoint,
            model,
            no_stream,
        } => {
            // Read full prompt from stdin
            let mut prompt = String::new();
            io::stdin().read_to_string(&mut prompt)?;
            if prompt.trim().is_empty() {
                anyhow::bail!("Empty prompt on stdin");
            }
            raw_completion(prompt, endpoint, model, no_stream).await?;
        }
        Command::Note {
            observation,
            modality,
            endpoint,
            model,
            no_stream,
        } => {
            let system = build_system_prompt(&modality);
            let prompt = format!(
                "Write a session note for today's session.\n\nObservation: {}",
                observation
            );

            let request = GenerateRequest {
                model,
                prompt,
                system,
                stream: !no_stream,
            };

            let client = Client::new();
            let url = format!("{}/api/generate", endpoint);

            let start = std::time::Instant::now();

            if no_stream {
                let resp: serde_json::Value = client
                    .post(&url)
                    .json(&request)
                    .send()
                    .await?
                    .json()
                    .await?;

                let text = resp["response"].as_str().unwrap_or("");
                println!("{}", text);

                let elapsed = start.elapsed();
                eprintln!(
                    "\n---\nGenerated in {:.1}s",
                    elapsed.as_secs_f64()
                );
            } else {
                let resp = client.post(&url).json(&request).send().await?;
                let mut stream = resp.bytes_stream();
                let mut total_tokens = 0u64;

                let stderr = io::stderr();
                let stdout = io::stdout();
                let mut out = stdout.lock();

                while let Some(chunk) = stream.next().await {
                    let bytes = chunk?;
                    if let Ok(parsed) = serde_json::from_slice::<StreamChunk>(&bytes) {
                        if let Some(text) = &parsed.response {
                            write!(out, "{}", text)?;
                            out.flush()?;
                        }
                        if parsed.done == Some(true) {
                            if let (Some(ec), Some(ed)) =
                                (parsed.eval_count, parsed.eval_duration)
                            {
                                total_tokens = ec;
                                let elapsed = start.elapsed();
                                let tps = if ed > 0 {
                                    ec as f64 / (ed as f64 / 1e9)
                                } else {
                                    0.0
                                };
                                let mut err = stderr.lock();
                                writeln!(err)?;
                                writeln!(err, "---")?;
                                writeln!(
                                    err,
                                    "Generated {} tokens in {:.1}s ({:.0} tok/s)",
                                    total_tokens,
                                    elapsed.as_secs_f64(),
                                    tps
                                )?;
                            }
                        }
                    }
                }
                println!();
            }
        }
    }

    Ok(())
}
