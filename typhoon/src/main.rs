use core::{error::TyphoonError};
use structopt::StructOpt;
use core::program::Program;

#[derive(Debug, StructOpt)]
#[structopt(name = "typhoon")]
enum Opts {
    Build {
        #[structopt(name = "FILE")]
        filename: String,
        #[structopt(short, long)]
        debug: bool
    },

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
        Opts::Build {filename, debug} => {
            let program = Program::new(filename);
            let result = program.as_binary_output(debug);
            match result {
                Ok(ret) => {dbg!(ret);}
                Err(e) => {eprintln!("{}", e);}
            };
        }
        _ => unimplemented!()
    }

    //
    // match opt {
    //     Opts::LLIR { filename } => {
    //         let mut program = Program::new(filename)?;
    //         let x1 = program.as_llir();
    //         println!("{}", x1);
    //         Ok(())
    //     }
    //     Opts::Ast { filename } => {
    //         let program = Program::new(filename).unwrap();
    //         dbg!(program.token_tree);
    //         Ok(())
    //     }
    //     Opts::Run { filename } => {
    //         let mut program = Program::new(&filename)?;
    //         let option = Path::new(&filename).file_name().unwrap().to_str().unwrap();
    //         let x = program.as_binary_output(option);
    //         match x {
    //             Ok((ec, stdout, stderr)) => {
    //                 println!("\nExitCode: {}", ec);
    //                 println!("\nSTDOUT:\n {}", stdout);
    //                 println!("\nSTDERR:\n {}", stderr);
    //                 Ok(())
    //             }
    //             Err(e) => Err(e),
    //         }
    //     }
    // }
    Ok(())
}
