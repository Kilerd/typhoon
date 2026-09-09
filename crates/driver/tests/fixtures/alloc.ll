; Hand-written fixture proving Boehm GC is linked and usable: 200 x 1 MiB
; allocations, each written to at both ends, none retained -- so the collector
; must actually run. Expected stdout: "200\n", exit 0.

declare void @ty_rt_init()
declare ptr @ty_alloc(i64)
declare ptr @ty_alloc_atomic(i64)
declare void @ty_print_int(i64)
declare void @ty_print_end()

define i32 @main() {
entry:
  call void @ty_rt_init()
  br label %loop

loop:
  %i = phi i64 [ 0, %entry ], [ %i.next, %loop ]
  %traced = call ptr @ty_alloc(i64 1048576)
  store i8 65, ptr %traced, align 1
  %traced.last = getelementptr inbounds i8, ptr %traced, i64 1048575
  store i8 90, ptr %traced.last, align 1
  %atomic = call ptr @ty_alloc_atomic(i64 1048576)
  store i8 65, ptr %atomic, align 1
  %atomic.last = getelementptr inbounds i8, ptr %atomic, i64 1048575
  store i8 90, ptr %atomic.last, align 1
  %i.next = add i64 %i, 1
  %done = icmp eq i64 %i.next, 200
  br i1 %done, label %exit, label %loop

exit:
  call void @ty_print_int(i64 200)
  call void @ty_print_end()
  ret i32 0
}
