use structopt::StructOpt;
use typhoon::program::Program;
use typhoon::error::TyphoonError;

extern crate typhoon;


#[derive(Debug, StructOpt)]
#[structopt(name = "typhoon")]
struct Opt {
    /// Activate debug mode
    // short and long flags (-d, --debug) will be deduced from the field's name
    #[structopt(short, long)]
    debug: bool,

    #[structopt(short, long)]
    ast: bool,

    /// File name: only required when `out` is set to `file`
    #[structopt(name = "FILE")]
    file_name: String,
}

fn main() -> Result<(), TyphoonError> {
    env_logger::init();
    let opt: Opt = Opt::from_args();

    let mut program = Program::new(opt.file_name)?;

    if opt.ast {
        dbg!(&program.token_tree);
    }
    if opt.debug {
        let llir = program.as_llir();
        println!("\nllir: \n{}", llir);
    }

    match program.as_binary_output("out") {
        Ok((ec, stdout, stderr)) => {

            println!("\nExitCode: {}", ec);
            println!("\nSTDOUT:\n {}", stdout);
            println!("\nSTDERR:\n {}", stderr);

            Ok(())
        }
        Err(e) => Err(e)
    }
}