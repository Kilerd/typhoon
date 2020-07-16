; ModuleID = 'typhoon'
source_filename = "typhoon"

define i32 @main() {
entry:
  %a = alloca i32
  store i32 1, i32* %a
  %load_ = load i32, i32* %a
  %b = alloca i32
  store i32 %load_, i32* %b
  %load_1 = load i32, i32* %a
  ret i32 %load_1
}