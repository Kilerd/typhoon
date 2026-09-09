; Hand-written fixture for `ty_panic`: buffered stdout must be flushed first,
; then `panic: index out of range` on stderr, then exit code 101.

@.str.before = private unnamed_addr constant [6 x i8] c"before", align 1
@.str.panic = private unnamed_addr constant [18 x i8] c"index out of range", align 1

declare void @ty_rt_init()
declare void @ty_print_str(ptr, i64)
declare void @ty_print_end()
declare void @ty_panic(ptr, i64) #0

define i32 @main() {
entry:
  call void @ty_rt_init()
  call void @ty_print_str(ptr @.str.before, i64 6)
  call void @ty_print_end()
  call void @ty_panic(ptr @.str.panic, i64 18)
  unreachable
}

attributes #0 = { noreturn }
