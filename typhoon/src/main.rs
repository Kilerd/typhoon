use core::{error::TyphoonError, program::Program};
use std::path::Path;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "typhoon")]
enum Opts {
    LLIR {
        #[structopt(name = "FILE")]
        filename: String,
    },
    Run {
        #[structopt(name = "FILE")]
        filename: String,
    },
    Ast {
        #[structopt(name = "FILE")]
        filename: String,
    },
}

fn main() -> Result<(), TyphoonError> {
    env_logger::init();
    let opt: Opts = Opts::from_args();

    match opt {
        Opts::LLIR { filename } => {
            let mut program = Program::new(filename)?;
            let x1 = program.as_llir();
            println!("{}", x1);
            Ok(())
        }
        Opts::Ast { filename } => {
            let program = Program::new(filename)?;
            dbg!(program.token_tree);
            Ok(())
        }
        Opts::Run { filename } => {
            let mut program = Program::new(&filename)?;
            let option = Path::new(&filename).file_name().unwrap().to_str().unwrap();
            let x = program.as_binary_output(option);
            match x {
                Ok((ec, stdout, stderr)) => {
                    println!("\nExitCode: {}", ec);
                    println!("\nSTDOUT:\n {}", stdout);
                    println!("\nSTDERR:\n {}", stderr);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    }
}
