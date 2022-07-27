# typhoon
a tentitive language to provide powerful development efficiency.

## language goals
 - static typed and strongly typed
 - ownership model and easy lifetime controll provided by GC
 - gentle learning curve
 - fast performance

## project modules
 - `ast` the abstract syntax tree for typhoon
 - `typhoon` Command line tool to execute compiler
 - `core` compile AST into LLIR or binary code
 - `parser` peg parser of typhoon
 - `llvm-wrapper` provide a simple and safe wrapper for llvm