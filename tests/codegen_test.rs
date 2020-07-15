use typhoon::program::Program;


fn run_test_with_expected(name: &str, program_text: &str, exit_code: i32, stdout: &str, stderr: &str)
{
    env_logger::try_init();
    let test_name = format!("output/{}", name);
    let mut program = Program::new_with_string(program_text.to_string()).unwrap();
    println!("llir: \n{}", program.as_llir());
    let result = program.as_binary_output(test_name.as_str()).unwrap();
    assert_eq!(exit_code, result.0.code().unwrap());
    assert_eq!(stdout, result.1);
    assert_eq!(stderr, result.2);

    delete_output_file(test_name.as_str());
}

fn delete_output_file(name: &str) {
    let string = format!("{}.o", name);
    let path1 = std::path::Path::new(string.as_str());
    std::fs::remove_file(path1);
    let path1 = std::path::Path::new(name);
    std::fs::remove_file(path1);
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