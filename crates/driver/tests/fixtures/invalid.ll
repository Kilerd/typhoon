; Deliberately broken IR: `%missing` is never defined. Used to check that a
; failing `clang` invocation surfaces as DriverError::Link with clang's own
; message.

define i32 @main() {
entry:
  %x = add i32 1, %missing
  ret i32 %x
}
