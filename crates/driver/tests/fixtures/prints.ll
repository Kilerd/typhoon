; Hand-written fixture covering every `ty_print_*` entry point and the
; `print(a, b)` lowering (print_a, sep, print_b, end). See DESIGN section 4.6.
; Expected stdout:
;   -42
;   5.0
;   True
;   False
;   None
;   7 ok
; exit 0.

@.str.ok = private unnamed_addr constant [2 x i8] c"ok", align 1

declare void @ty_rt_init()
declare void @ty_print_int(i64)
declare void @ty_print_float(double)
declare void @ty_print_bool(i8)
declare void @ty_print_str(ptr, i64)
declare void @ty_print_none()
declare void @ty_print_sep()
declare void @ty_print_end()

define i32 @main() {
entry:
  call void @ty_rt_init()

  ; print(-42)
  call void @ty_print_int(i64 -42)
  call void @ty_print_end()

  ; print(5.0) -- must print `5.0`, never `5`
  call void @ty_print_float(double 5.000000e+00)
  call void @ty_print_end()

  ; print(True) / print(False)
  call void @ty_print_bool(i8 1)
  call void @ty_print_end()
  call void @ty_print_bool(i8 0)
  call void @ty_print_end()

  ; print(None)
  call void @ty_print_none()
  call void @ty_print_end()

  ; print(7, "ok")
  call void @ty_print_int(i64 7)
  call void @ty_print_sep()
  call void @ty_print_str(ptr @.str.ok, i64 2)
  call void @ty_print_end()

  ret i32 0
}
