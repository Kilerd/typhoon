

pub mod parser;





#[cfg(test)]
mod test {
    use ast::{ModuleItem, Type};

    use crate::parser::parse_module;

    #[test]
    fn test_empty_struct() {
        let result = parse_module(r#"
            struct Empty {
            }
        "#).unwrap();

        assert_eq!(result.items.len(), 1);
        match &*result.items[0] {
            ModuleItem::StructDeclare(s) => {
                assert_eq!(s.name, "Empty");
                assert_eq!(s.fields.len(), 0);
            },
            _ => panic!("Expected struct declaration")
        }
    }
    #[test]
    fn test_struct_one_field() {
        let result = parse_module(r#"
            struct Single {
                value: i32
            }
        "#).unwrap();

        assert_eq!(result.items.len(), 1);
        match &*result.items[0] {
            ModuleItem::StructDeclare(s) => {
                assert_eq!(s.name, "Single");
                assert_eq!(s.fields.len(), 1);
                assert_eq!(s.fields["value"], Type::named("i32".to_string()));
            },
            _ => panic!("Expected struct declaration")
        }
    }
    #[test]
    fn test_struct_define() {
        let result = parse_module(r#"
            struct Point {
                x: i32,
                y: i32,
            }
        "#).unwrap();

        assert_eq!(result.items.len(), 1);
        match &*result.items[0] {
            ModuleItem::StructDeclare(s) => {
                assert_eq!(s.name, "Point");
                assert_eq!(s.fields.len(), 2);
                assert_eq!(s.fields["x"], Type::named("i32".to_string()));
                assert_eq!(s.fields["y"], Type::named("i32".to_string())); 
            },
            _ => panic!("Expected struct declaration")
        }
    }

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