; Hand-written fixture: the shape `typhoon-codegen` will emit for examples/hello.ty.
; LLVM 18+ syntax: opaque pointers (`ptr`), no `i8*`.
; Expected stdout: "Hello, Typhoon!\n", exit 0.

@.str.hello = private unnamed_addr constant [15 x i8] c"Hello, Typhoon!", align 1

declare void @ty_rt_init()
declare void @ty_print_str(ptr, i64)
declare void @ty_print_end()

define i32 @main() {
entry:
  call void @ty_rt_init()
  call void @ty_print_str(ptr @.str.hello, i64 15)
  call void @ty_print_end()
  ret i32 0
}
