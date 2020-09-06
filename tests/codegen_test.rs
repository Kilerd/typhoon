
use std::process::Command;
use typhoon::program::Program;

fn run_test_with_expected(
    name: &str,
    program_text: &str,
    exit_code: i32,
    stdout: &str,
    stderr: &str,
) {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut program = Program::new_with_string(program_text.to_string()).unwrap();

    let llir = program.as_llir();
    let llir_file = format!("output/{}.ll", name);
    std::fs::write(&llir_file, llir).unwrap();

    let output = Command::new("lli")
        .arg(&llir_file)
        .output()
        .expect("failed to execute llir");
    let runtime_exit_code = output.status.code().unwrap();
    let runtime_stdout = String::from_utf8(output.stdout).unwrap();
    let runtime_stderr = String::from_utf8(output.stderr).unwrap();

    assert_eq!(exit_code, runtime_exit_code);
    assert_eq!(stdout, runtime_stdout);
    assert_eq!(stderr, runtime_stderr);
}

#[test]
fn return_constant_i8() {
    let t = r#"
    fn main() -> i8 {
        return 1i8;
    }
    "#;

    run_test_with_expected("return_constant_i8", t, 1, "", "");
}

#[test]
fn return_constant_i32() {
    let t = r#"
    fn main() -> i32 {
        return 3i32;
    }
    "#;
    run_test_with_expected("return_constant_i32", t, 3, "", "");
}

#[test]
fn return_i32_from_variable_assigned() {
    let t = r#"
    fn main() -> i32 {
        let a: i32 = 10;
        return a;
    }
    "#;
    run_test_with_expected("return_i32_from_variable_assigned", t, 10, "", "");
}

#[test]
fn return_i32_from_variable_assigned_multiple() {
    let t = r#"
    fn main() -> i32 {
        let a: i32 = 10;
        let b:i32 = a;
        return b;
    }
    "#;
    run_test_with_expected("return_i32_from_variable_assigned_multiple", t, 10, "", "");
}

#[test]
fn load_struct_value_as_return_code() {
    let t = r#"
    struct A {
        inner: i32,
    }
    fn main() -> i32 {
        let a: A = A {inner: 4};
        return a.inner;
    }
    "#;
    run_test_with_expected("load_struct_value_as_return_code", t, 4, "", "");
}

#[test]
fn load_nested_struct_value_as_return_code() {
    let t = r#"
    struct A {
        inner: i32,
    }
    struct B {
        inner: A,
    }
    fn main() -> i32 {
        let a: A = A {inner: 4};
        let b: B = B {inner: a};
        return b.inner.inner;
    }
    "#;
    run_test_with_expected("load_nested_struct_value_as_return_code", t, 4, "", "");
}
