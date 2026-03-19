use clap::{Parser, Subcommand};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a new ed25519 key pair. Outputs the secret key as hex.
    Gen,
    /// Output the public key for a given secret key.
    Pub {
        /// Secret key as hex.
        #[arg(value_parser = parse_signing_key)]
        skey: SigningKey,
    },
    /// Sign a message with a secret key.
    Sign {
        /// Secret key as hex.
        #[arg(value_parser = parse_signing_key)]
        skey: SigningKey,
        /// Message as hex.
        #[arg(value_parser = parse_hex)]
        msg: HexBytes,
    },
    /// Verify a signature against a public key and message.
    Verify {
        /// Public key as hex.
        #[arg(value_parser = parse_verifying_key)]
        pkey: VerifyingKey,
        /// Message as hex.
        #[arg(value_parser = parse_hex)]
        msg: HexBytes,
        /// Signature as hex.
        #[arg(value_parser = parse_sig)]
        sig: Signature,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Gen => {
            let skey = SigningKey::generate(&mut OsRng);
            println!("{}", hex::encode(skey.to_bytes()));
        }
        Cmd::Pub { skey } => {
            println!("{}", hex::encode(skey.verifying_key().to_bytes()));
        }
        Cmd::Sign { skey, msg } => {
            let sig = skey.sign(&msg.0);
            println!("{}", hex::encode(sig.to_bytes()));
        }
        Cmd::Verify { pkey, msg, sig } => match pkey.verify(&msg.0, &sig) {
            Ok(()) => println!("valid"),
            Err(_) => {
                println!("invalid");
                std::process::exit(1);
            }
        },
    }
}

#[derive(Clone)]
struct HexBytes(Vec<u8>);

fn strip_quotes(s: &str) -> &str {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}

fn parse_hex(s: &str) -> Result<HexBytes, String> {
    hex::decode(strip_quotes(s)).map(HexBytes).map_err(|e| e.to_string())
}

fn parse_signing_key(s: &str) -> Result<SigningKey, String> {
    let bytes: [u8; 32] = hex::decode(strip_quotes(s)).map_err(|e| e.to_string())?.try_into().map_err(|_| "must be 32 bytes".to_string())?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn parse_verifying_key(s: &str) -> Result<VerifyingKey, String> {
    let bytes: [u8; 32] = hex::decode(strip_quotes(s)).map_err(|e| e.to_string())?.try_into().map_err(|_| "must be 32 bytes".to_string())?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| e.to_string())
}

fn parse_sig(s: &str) -> Result<Signature, String> {
    let bytes: [u8; 64] = hex::decode(strip_quotes(s)).map_err(|e| e.to_string())?.try_into().map_err(|_| "must be 64 bytes".to_string())?;
    Ok(Signature::from_bytes(&bytes))
}
