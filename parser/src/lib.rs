pub mod parser;







#[cfg(test)]
mod test {
    use crate::parser::parse_module;

    #[test]
    fn test() {
        let result = parse_module(r#"
            struct A {
                inner: i32,
            }
            struct B {
                inner: A,
            }
            fn main() -> i32 {
                let a: i32 = {
                            let b : i8 = 1i8+{1};
                            b+1-1
                        };
                return a.b.c(1,{a},);
                    {c}
            }
        "#).unwrap();
        let x = &result.items[0];
        dbg!(result);
    }
    #[test]
    fn test_print() {
        let result = parse_module(r#"
            fn main() -> () {
                print("hello world");
            }
        "#);
        dbg!(result);
    }
}