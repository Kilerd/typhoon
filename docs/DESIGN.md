# Typhoon 语言设计（v0 草案，2026-09-09）

> 状态：v0 草案，对应项目重启后的第一版设计。
> 除第 9 节「开放问题」外，本文所有条目均已定稿，实现时以本文为准；要改需要先改本文。
> 约定：散文用中文，标识符、关键字、代码、错误信息一律用英文。

---

## 1. 定位与目标

### 1.1 一句话定位

> **Python 的语法，Go/Nim 的语义，Rust 写的工具链，输出单二进制。**

### 1.2 我们要做的事

- **Python 风味语法**：缩进块、`:` 引导块、`class`、`for x in ...`、推导式、f-string。写过 Python 的人应该能在十分钟内读懂 typhoon 代码。
- **静态强类型**：所有类型在编译期确定。函数签名必须显式标注，函数体内的局部变量由编译器推断。没有 `Any`，没有由运行时类型驱动的动态分发。
- **AOT 编译为原生单二进制**：`typhoon build main.ty` 产出一个静态链接（含 runtime 与 GC）的可执行文件。部署时不需要解释器、不需要虚拟机、不需要 `site-packages`。
- **性能对标 Go**：单线程基准与 Go 打平，详见第 2 节。

### 1.3 路线选择：Python 风味，而不是 Python 兼容

这是整个项目最重要的决定，它决定了后面所有取舍。业界让 Python「变快」的项目大致分两类：一类保 Python 兼容性，一类只保语法观感。

| 项目 | 路线 | 代价 |
|---|---|---|
| mypyc | 编译 Python 子集，保留 CPython 对象模型 | 受制于 `PyObject`、引用计数与 GIL，加速有天花板 |
| Codon | 重新实现 Python 语义的静态编译器，追求源码基本兼容 | 兼容性长尾无穷无尽，语义偏差长期咬人 |
| Mojo | Python 超集 + 系统级新语法 | 同时背负 CPython 互操作与一整套新语义两份复杂度 |
| Nim | Python 风味缩进语法 + 完全自有的静态语义 | 与 Python 生态不互通，生态要自己长 |
| **typhoon** | **同 Nim：Python 风味语法 + 自有静态语义** | **不兼容 Python 生态，但语义从第一天起就为性能与静态检查设计** |

typhoon 走 Nim 那条路：**只借语法，不借语义，也不借生态。**

好处是编译器永远不必为 `__getattr__`、`eval`、猴子补丁、元类这类动态特性留后门，对象布局、方法分发、内存表示可以完全按性能来设计；坏处是没有 pip 生态可用，标准库必须自己长出来。这个交换是有意识做出的，不再讨论。

### 1.4 非目标（Non-goals）

明确写下来，以后有人提议时直接引用本节：

- **不追求 CPython 兼容**：typhoon 不承诺任何 `.py` 文件可以原样编译通过。
- **不支持 `eval` / `exec` / 运行时代码生成。**
- **不支持猴子补丁**：类、方法、函数在编译期定型，运行时不可替换。
- **不支持元类（metaclass）与 `__getattr__` / `__setattr__` 拦截。**
- **不支持运行时给对象增删属性**：对象布局编译期固定，字段访问编译为常量偏移。
- **不做 CPython C 扩展（CPython ABI）互操作**：numpy / pandas 不能直接用。普通 C FFI 是另一回事，M4 提供。
- **`int` 不是大整数**：`int` 就是 64 位有符号整数，溢出回绕。需要任意精度时用库类型（未来）。

---

## 2. 性能目标：单线程对标 Go

目标不是「比 Python 快」——那是必然的、没有信息量的目标——而是**在单线程基准上与 Go 打平**。Go 是一个诚实的参照物：同样是 GC 语言、同样 AOT、同样不做全程序特化，且它的编译产物是业界公认「够快」的基线。凡是 typhoon 明显慢于 Go 的地方，都说明我们的实现有真问题，而不是语言设计的必然代价。

「里程碑」一列表示该目标从哪个里程碑开始成为**退出标准**（exit criterion），在此之前只观测、不阻塞。

| 基准 | 类型 | 目标（相对 Go 耗时） | 里程碑 |
|---|---|---|---|
| fib(35)、循环求和 | 纯 CPU、递归 | ≤ 1.0x | M1 |
| nbody、mandelbrot、spectral-norm | 浮点数值 | ≤ 1.0x | M1 |
| fannkuch-redux | 整数数组 | ≤ 1.0x | M2 |
| binary-trees | 分配密集 | ≤ 2.0x（Boehm 阶段）→ ≤ 1.0x（逃逸分析 + 新 GC 后） | M2 → M5 |
| 字符串拼接/fasta | 字符串 | ≤ 1.5x → ≤ 1.0x | M2 → M4 |
| k-nucleotide | dict 密集 | ≤ 1.5x → ≤ 1.0x | M3 → M4 |
| 并发基准 | goroutine 风格 | 未定义，见开放问题 | M6+ |

### 2.1 为什么纯 CPU 基准打平是合理预期

在 CPU 密集、几乎不分配的代码上，**LLVM `-O2` 通常优于 Go 的 gc 编译器**：

- **自动向量化**：LLVM 有成熟的 loop vectorizer / SLP vectorizer，gc 编译器基本没有。
- **激进内联与过程间优化**：LLVM 的内联启发式、常量传播、循环不变量外提都比 gc 更进一步；gc 出于编译速度考虑刻意保守。
- **泛型实现方式**：typhoon 做**单态化**（每个类型实参生成一份特化代码），Go 的泛型用 GC shape stenciling + dictionary，指针类型共享同一份代码并带字典间接层，数值代码上会吃亏。
- **语义上的让路**：`int` 固定为 i64 且溢出回绕（无溢出检查分支）；`range` 循环编译为计数循环、循环体内不做边界检查（索引由编译器证明在界内）。这两条使内层循环能干净地降低为 LLVM 的规范循环形式。

因此从 M1 起，fib、nbody、mandelbrot、spectral-norm 这类基准打平 Go 是**预期结果**而不是挑战目标；做不到说明是我们的 lowering 有问题（例如没有把局部变量放进寄存器、每次运算都走 runtime 调用）。

### 2.2 Go 难打平的地方：分配密集型代码

真正难的是分配密集的场景（binary-trees 是典型）。Go 在这里有三件我们 MVP 阶段没有的武器：

| 能力 | Go | typhoon MVP |
|---|---|---|
| 逃逸分析 | 有，非逃逸对象直接栈分配 | 无，全部堆分配 |
| 分配路径 | 每 P 私有的 mcache，指针碰撞式快速路径，无锁 | Boehm 的通用分配器 |
| GC | 精确、并发标记、写屏障，STW 只有亚毫秒 | Boehm 保守式、stop-the-world 标记清扫 |

要在这类基准上追平 Go，需要按顺序做三件事：

1. **在 IR 上做逃逸分析**，把不逃逸的对象改为栈分配（M5）。这是收益最大的一步——binary-trees 里绝大多数节点其实是短命的。
2. **替换 Boehm**：评估 RC + move 语义（Nim ORC 模型，编译期插入引用计数、循环用 cycle collector 兜底）或精确分代 GC。Boehm 的保守扫描既拖慢标记，也会造成不确定的内存滞留。
3. **调优 runtime 数据结构**：dict 用 swiss-table 布局（开放寻址 + SIMD 探测），`str` 用 Rust 风格的 `{ptr, len}` 不可变切片 + rope/builder 式拼接，避免 O(n²) 的朴素拼接。

字符串与 dict 基准的目标之所以分两阶段（M2/M3 先 ≤1.5x，M4 再 ≤1.0x），正是因为第 3 步排在 M4。

### 2.3 并发

并发是独立子系统，模型尚未选定（见第 9 节），**M6 之前不在范围内**，也不设性能目标。需要注意的是：并发模型的选择会反向约束 GC 与栈的设计（例如 goroutine 风格的可增长栈与保守 GC 基本冲突），所以在 M5 决定 GC 方案时必须把这一点一并考虑。

### 2.4 测量方法

任何性能声明都必须来自 `bench/run.sh` 的实测（见第 7 节）：同一台机器、同一次运行、各跑 N 次取中位数、报告相对 Go 的比值。禁止凭「LLVM 应该更快」下结论。基准对照的 Go 版本固定在 `bench/GO_VERSION`，升级时同步更新。

---

## 3. 语法

以下每个特性给一个最小示例。完整可运行示例见 `examples/` 目录。

### 3.1 词法与块结构

- 缩进表示块，块头以 `:` 结尾；推荐 4 空格，**不接受 tab**（词法阶段直接报错，杜绝混用）。
- 注释用 `#`，到行尾结束。没有块注释。
- 语句以换行结束，**不写分号**。
- 括号 `()` `[]` `{}` 内部换行为隐式续行，不产生 NEWLINE / INDENT。

```python
# this is a comment
fn main():
    total = 0
    for i in range(10):
        total = total + i
    print(total)
```

### 3.2 函数

关键字是 `fn` 而不是 `def`。选 `fn` 的理由：`def` 在读者心里绑定了「运行时定义一个可被替换的对象」，而 typhoon 的函数是编译期符号；换个关键字提前告诉读者语义不同。

```python
fn add(a: int, b: int) -> int:
    return a + b

# no `->` means the function returns unit (None)
fn log(message: str):
    print(message)

# default argument
fn scale(x: float, factor: float = 1.0) -> float:
    return x * factor

fn main():
    print(add(1, 2))            # positional
    print(add(a=1, b=2))        # keyword arguments
    print(scale(2.0))           # uses the default
```

- 参数类型**必须**标注（MVP 不做参数类型推断）。
- 返回类型省略即返回 unit；unit 的值写作 `None`。
- 默认值必须是编译期常量表达式。
- 关键字实参在调用点按名字匹配形参，求值顺序仍是书写顺序。

### 3.3 程序结构与入口

- 每个程序必须有 `fn main():`，它是入口；返回 unit。
- **顶层只允许声明**：`fn`、`class`、`import`、常量。**不允许顶层语句**（没有 `if __name__ == "__main__"` 这种东西，也不存在模块级副作用）。

```python
PI: float = 3.14159

fn area(r: float) -> float:
    return PI * r * r

fn main():
    print(area(2.0))
```

顶层常量必须带类型标注，且初始值为常量表达式；它们不是变量，不可赋值。常量表达式的短路与运行时一致：比较链在第一个不成立的环节停下，`and` / `or` 在左操作数已决定结果时停下；没有被求值的部分仍做类型检查，但其中的除零、负指数、负移位不算错误。

### 3.4 局部变量与作用域

```python
fn main():
    x = 1              # first assignment declares `x` and infers `int`
    y: float = 2.0     # explicit annotation
    x = x + 1          # ok, still int
    # x = "hello"      # error: cannot assign str to variable of type int
```

规则：

- **首次赋值即声明**，类型由初值推断；也可以显式标注。
- 类型在首次赋值时**固定**，后续赋值必须是同一类型。
- **函数作用域**（Python 语义），不是块作用域：`if`/`for` 体内引入的变量在函数其余部分可见。
- **定值分析（definite assignment）**：编译器保证变量在所有到达其读取点的路径上都已被赋值，否则报错——只在 `if` 的一个分支里赋值、随后就读取，属于编译错误（`v` may be uninitialized here）。这替代了 Python 的 `UnboundLocalError` 运行时异常。
- **不允许遮蔽（shadowing）**：同一函数内不能重复声明同名变量，内层块也不行。理由是缩进语法下遮蔽的可读性极差。

### 3.5 控制流

```python
fn classify(n: int) -> str:
    if n < 0:
        return "negative"
    elif n == 0:
        return "zero"
    else:
        return "positive"

fn main():
    i = 0
    while i < 5:
        i = i + 1

    for i2 in range(10):        # 0..9
        if i2 % 2 == 0:
            continue
        if i2 > 6:
            break

    for x in range(0, 10, 2):   # start, stop, step
        print(x)

    for item in ["a", "b"]:     # iterate any iterable
        print(item)
```

`range(n)` / `range(a, b)` / `range(a, b, step)` 是语言内建形式，编译为计数循环，**不分配对象**。下一个值越过 `int` 的表示范围时循环结束（与 Python 一致，不回绕成死循环）；步长为 `1` / `-1` 时循环条件已保证不会溢出，生成的代码不带检查，其余步长每轮多一次溢出判断。`for ... in` 作用于 `list`、`str`、`dict`（迭代 key）、`set` 与 `range`。对 `str` 迭代按 Unicode 码点进行（整体 O(n)，与按码点索引的 O(n)/次不同）；迭代产出的元素类型（独立的 `char` 类型还是长度为 1 的 `str`）随第 9 节第 5 条一并决定。MVP 没有 `else` 子句挂在循环上（Python 的 `for...else` 不引入）。

### 3.6 运算符

| 类别 | 运算符 |
|---|---|
| 算术 | `+` `-` `*` `/` `//` `%` `**` |
| 比较 | `==` `!=` `<` `<=` `>` `>=` |
| 身份 | `is` `is not`（用于 `None` 与引用相等） |
| 逻辑 | `and` `or` `not`（短路求值，操作数必须是 `bool`） |
| 成员 | `in` `not in`（`list` / `dict` / `set` / `str`） |
| 位运算 | `&` `\|` `^` `<<` `>>` `~`（仅整数类型） |

优先级（由低到高，Pratt 解析器按此实现）：

```
or  <  and  <  not  <  comparison/in/is  <  |  <  ^  <  &  <  << >>  <  + -  <  * / // %  <  unary - + ~  <  **  <  call/index/attribute
```

- `**` 右结合，其余二元运算符左结合。
- `/` 作用于两个 `int` 时结果是 `float`（Python 语义）；`//` 是整除，结果类型与操作数一致。
- `and` / `or` 的操作数必须是 `bool`，**没有 truthiness**：`if xs:` 不合法，要写 `if len(xs) > 0:`。这是刻意的，避免为每个类型定义「真值」语义。
- 比较链 `a < b < c` 计划在 M3 支持，MVP 需要写成 `a < b and b < c`。

### 3.7 字面量

```python
fn main():
    i = 42                      # int
    f = 3.14                    # float
    s = "double quoted"         # str
    s2 = 'single quoted'        # str, same type
    msg = f"i={i} f={f:.2f}"    # f-string with format spec
    t = True                    # bool
    n = None                    # unit / absence
    xs: list<int> = [1, 2]
    d: dict<str, int> = {"a": 1}
    pair: tuple<int, str> = (1, "a")
    st: set<int> = {1, 2}
```

- 空 `{}` 是空 dict；空 set 用 `set()`。
- f-string 支持 `{expr}` 与 `{expr:spec}`，`spec` 采用 Python 格式迷你语言的子集（MVP：`.Nf`、`d`、`s`）。f-string 在编译期展开为拼接/格式化调用，不做运行时解析。`{expr}` 接受 `print` 能打印的任何值（M3 起包括容器，文本与 `print` 写出的完全一致，即 `repr(x)`；容器里的类要有 `__repr__`），`spec` 只用于标量。
- 数字字面量支持 `_` 分隔符（`1_000_000`）与 `0x` / `0b` / `0o` 前缀。

### 3.8 class

字段用注解声明，编译器**自动生成关键字构造器**（dataclass 风格）。方法第一个参数是 `self`（不标注类型）。

```python
class Point:
    x: float
    y: float

    fn dist(self) -> float:
        return (self.x * self.x + self.y * self.y) ** 0.5

    fn translate(self, dx: float, dy: float):
        self.x = self.x + dx
        self.y = self.y + dy

fn main():
    p = Point(x=3.0, y=4.0)     # generated keyword constructor
    print(p.dist())
```

- 构造器**只接受关键字实参**，顺序无关，缺一不可（有默认值的字段可省略）。
- 字段可以带默认值：`count: int = 0`。
- **没有继承**（M3 起为既定决定）；`class C(T1, T2):` 括号里只能写 trait（见 3.11），写类名直接报错 `inheritance is not supported`。
- 没有 `__init__` 自定义钩子（MVP）；需要校验逻辑时写工厂函数 `fn make_point(...) -> Point:`。
- 实例是引用类型，分配在 GC 堆上；字段全是标量时用 `ty_alloc_atomic` 分配，GC 不扫描该块。

**M2 提前放开的可空类引用。** 第 3.10 节把 `T | None` 排在 M3，但没有它就写不出自引用的数据结构（binary-trees 的 `Node`），所以 M2 提前实现了它的一个切片：**仅 class 类型**可以写成 `C | None`，可出现在字段、参数、返回值与局部变量上，表示一个可能为空的指针；`None` 字面量可以赋给它，`C` 可以隐式加宽为 `C | None`（两者都是一个指针）。收窄靠 `is None` / `is not None`，且**只在 `if` 内生效**——`if x is None: ... else: ...` 与 `if x is None: return` 之后的早返回形式都能收窄；`and` / `or` 内部与循环条件不收窄，循环体若给该局部重新赋值（含嵌套块，以及把它用作 `for` 的循环变量）也会丢弃收窄结果，而且循环条件、循环体与循环之后三处都按丢弃后的状态检查——条件在每轮之后重新求值，循环体也可能执行零次或多次。未收窄就把 `C | None` 当 `C` 用是编译错误。`int | None` 等其余联合类型仍然留在 M3。

**M2 不提供的两件事。** `print(obj)` 与 `obj == obj` 都报错：前者提示「显式格式化字段」，后者提示「用 `is` 比较身份」。两者都要等 M3 的 repr / eq 协议定下来——一旦定了，容器的 repr 与相等性也要跟着走同一套规则，现在草率给一个 `Point(x=1.0)` 式的默认实现，将来会成为兼容性包袱。`obj is obj` 一直可用。M3 用 3.11 的 `Eq` / `Repr`（及其派生）解决了这两件事：类定义了 `__repr__` / `__eq__` 才能打印 / 比较。

### 3.9 泛型

采用尖括号形参列表（Rust / C# / TypeScript 风格），而不是 Python 的方括号写法，实现方式是**单态化**。

```python
fn max<T>(a: T, b: T) -> T:
    if a > b:
        return a
    return b

class Stack<T>:
    items: list<T>

    fn push(self, value: T):
        self.items.append(value)

    fn pop(self) -> T:
        return self.items.pop()

fn main():
    print(max(1, 3))            # T inferred as int
    print(max<float>(2.5, 1.5)) # explicit instantiation
    s: Stack<int> = Stack(items=[])
    s.push(1)
```

**为什么是尖括号，以及如何消歧**

1. **理由**：在 typhoon 里 `[]` 只表示索引，类型实参与值索引在视觉上彻底分开，读代码时不必先判断方括号里装的是值还是类型。Python 用方括号是因为它的泛型本质上是运行时的 `__class_getitem__` 调用（PEP 585 / PEP 695 都建立在这个机制上），而 typhoon 的类型是编译期实体，没有理由沿用这个历史包袱。
2. **类型上下文无歧义**：在 `:` 之后的注解、`->` 之后、`<...>` 内部、`class Name` 之后、`fn name` 之后这些位置，`<` 一律开启类型实参列表，不存在与小于号的歧义。
3. **表达式上下文采用 C# 规则**：遇到 `标识符 <` 时先推测性地（speculatively）按类型实参列表解析，只有当列表整体能解析为类型、且配对的 `>` 后面紧跟 `(` 时，才判定为泛型调用，否则回退按比较运算解析。推论是 `a < b > (c)` 会被读成泛型调用 `a<b>(c)`；要表达比较请加括号写 `(a < b) > c`，或把左边的名字括起来写 `(a) < b > (c)`——括号括起的表达式不是标识符，即使里面只有一个名字（`obj.attr <` 以名字结尾，仍按本条处理）。MVP 不支持比较链，M3 引入比较链时沿用同一条规则。
4. **词法与嵌套**：词法器照常产出 `>>`、`>=`、`>>=` 这些复合 token；parser 在关闭嵌套的类型实参列表（如 `dict<str, list<int>>`）时把 `>>` 拆成两个 `>`。这是 C++ / Java / Rust 解析器的标准做法，也是坚持手写 parser 的又一个理由。

- 类型实参可由实参推断，也可显式写 `f<int>(...)`。
- MVP **没有约束语法**（`T: Comparable`）：对 `T` 使用了某个运算符时，在单态化后检查该具体类型是否支持；错误信息里附带实例化栈。这一模板语义在 M3 引入约束语法（`fn f<T: Ord>`，见 3.11）后仍保留给未加约束的 `T`。

### 3.10 分阶段引入的语法

| 特性 | 语法 | 里程碑 |
|---|---|---|
| 推导式 | `[x * x for x in xs if x > 0]`、dict/set 推导式 | M3 |
| 可空类型 | `T \| None`，配合 `is None` / `is not None` 做流敏感收窄 | M3 |
| 比较链 | `a < b < c` | M3 |
| 异常 | `try` / `except E as e` / `finally` / `raise` | M4 |
| 模块 | `import foo`、`from foo import bar` | M4 |
| 模式匹配 | `match` / `case` | 开放 |

```python
# M3
fn find(xs: list<int>, target: int) -> int | None:
    for i in range(len(xs)):
        if xs[i] == target:
            return i
    return None

fn main():
    r = find([1, 2, 3], 2)
    if r is None:
        print("not found")
    else:
        print(r)        # here `r` is narrowed to int
```

**M3 已落地的语义。**

- **`T | None`**：任何 `T` 都可以写成 `T | None`（`T | None | None` 即 `T | None`），除此之外的联合类型一律报错。表示不装箱：引用类型（`str`、`list<T>`、class）用空指针表示 `None`，与 `T` 同一布局；值类型（`int`、`float`、`bool`、tuple）是按值传递的 `{ tag: i8, T }` 对（wasm 上展开为标签加载荷的局部变量）。`T` 隐式加宽为 `T | None`，`None` 可赋给 `T | None`；收窄规则沿用 3.8 节（`if` 内与早返回形式，`and` / `or` 与循环不泄漏，重新赋值即丢弃），推导式的 `if` 子句同样收窄其后的子句与元素。未收窄就当 `T` 用是编译错误；`x == None` 报错并提示改用 `is None`；`T | None` 与 `T` 之间的 `==` 会先加宽 `T`。`print` / f-string 打印值或 `None`。
- **比较链**：`a < b < c` 即 `a < b and b < c`，但中间操作数只求值一次（必要时存入隐藏临时变量），从左到右短路；每一环按单个比较的规则检查（`==` `!=` `<` `<=` `>` `>=` `is` `is not` `in` `not in`），整条链不收窄。与 Python 一致，`x in y` / `x not in y` 先求值 `x` 再求值 `y`（单独使用或作为链中一环都如此）。`a < b > (c)` 仍按 3.9 节第 3 条读作泛型调用。
- **列表推导式**：`[expr for x in it if cond ...]`，`for` / `if` 子句任意多个，可迭代对象与 `for` 语句相同（`range(...)`、`list<T>`、`str`）。推导式变量的作用域限于推导式本身（Python 3 语义）：与外层变量或常量同名时只在推导式内部遮蔽它，外层绑定的值不受影响，推导式结束后名字恢复指向外层变量——这是 3.4 节「不允许遮蔽」唯一的例外，因为推导式变量不会泄漏到函数作用域。降低为普通的循环加 `append`；只有一个 `for` 子句、没有 `if`，且遍历 list、str 或步长为字面量的 `range` 时，结果按最终长度一次分配。
- **`dict<K, V>` 与 `set<T>`**：key（以及 set 的元素）是 `int`、`bool`、`str`、由它们组成的扁平 tuple，或实现了 3.11 的 `Hash`（因而也实现 `Eq`）的类——类必须**声明** `Hash`，只有 dunder 方法不算，泛型实例与带 `T: Hash` 约束的类型参数同样可以。`float` 不实现 `Hash`；含类成员的 tuple 与 `T | None` 作 key 在 M3 报错。迭代顺序就是插入顺序（CPython 3.7+ 的紧凑 dict），两个后端逐字节一致：覆盖已有 key 保留原位置，删除后再插入排到末尾。`{}` 是空 dict、`set()` 是空 set，两者都需要期望类型（与 `[]` 相同）；`dict()` 等同 `{}`。`d[k]` 缺 key 时 panic（`key not found: <key 的 repr>`；没有 `__repr__` 的类 key 写作 `<Point object>`），`k in d`、`len`、`d.get(k, default)`、`d.pop(k)` / `d.pop(k, default)`、`keys()` / `values()` / `items()`（返回新的 `list`）；`d.get(k)` 即 `d.get(k, None)`，默认值只能放进 `V | None` 时结果类型为 `V | None`，需要收窄。`set` 有 `add` / `remove`（缺失 panic）/ `discard`。`for k in d`、`for x in s` 与 `sorted(d)` 遍历 key 的快照，循环体内修改容器是良定义的。`print`、f-string 与 `str(x)` 输出 `{"a": 1}`、`{1, 2}`，空 set 输出 `set()`。dict / set 的 `==` 暂不支持。**与 Python 的已知差异**：`d[k] = v` 先求值 `d`、`k` 再求值 `v`（与 `xs[i] = v` 相同），Python 是先求值 `v`。
- **dict / set 推导式**：`{k: v for x in it if cond ...}` 与 `{e for x in it ...}`，子句、作用域与遮蔽规则与列表推导式完全相同；元素 / key 必须满足上面的 key 规则，dict 推导式先求值 key 再求值 value，重复的 key 保留最后一个 value。所有推导式与 `for` 语句一样可以遍历 dict（key）与 set。
- **`sorted(xs)`**：返回新 list，稳定排序；元素为 `int`、`float`、`bool`、`str` 或它们的扁平 tuple（按成员逐个比较，由运行时排序），或任何其他实现了 `Ord` 的类型——有 `__lt__` / `__eq__` 的类、含这种类的 tuple（见 3.11，单态化的归并排序，只调用 `__lt__`）；也接受 set 与 dict（排序其 key）。`float` 按 `<` 排序，NaN 排在最后。
- **`str` 切片 `s[a:b]`**：见第 9 节第 5 条。

### 3.11 trait 与运算符重载（M3，2026-10-12 决定，同日实现）

类型抽象采用**名义 trait**：一个类型满足某个 trait，当且仅当它显式声明了这一点，「恰好有同名方法」不算。**不引入继承。** 运算符重载通过 dunder 方法实现，**静态分派**。语法参照 Mojo（同为 Python 风味、名义 trait 的高性能语言）。

```python
trait Shape:
    fn area(self) -> float                # required: no body
    fn describe(self) -> str:             # default implementation, may be overridden
        return f"area {self.area()}"

class Circle(Shape):                      # the parentheses list traits, never a base class
    r: float

    fn area(self) -> float:
        return 3.14159 * self.r * self.r

fn total_area<T: Shape>(xs: list<T>) -> float:   # checked once, here
    s = 0.0
    for x in xs:
        s = s + x.area()
    return s

class Point(Eq, Ord, Hash, Repr):         # declared but not defined -> derived field-wise
    x: int
    y: int

class Vec(Repr):
    x: float
    y: float

    fn __mul__(self, k: float) -> Vec:    # `v * 2.0`; the operand follows the signature
        return Vec(x=self.x * k, y=self.y * k)

fn main():
    print(total_area([Circle(r=1.0), Circle(r=2.0)]))
    p = Point(x=1, y=2)
    print(p, p == Point(x=1, y=2), p < Point(x=1, y=3))   # Point(x=1, y=2) True True
    print([p], Vec(x=1.0, y=2.0) * 2.0)    # [Point(x=1, y=2)] Vec(x=2.0, y=4.0)
    seen: set<Point> = {p, Point(x=1, y=2), Point(x=0, y=5)}  # a key through Hash + Eq
    print(len(seen), Point(x=0, y=5) in seen, sorted(seen))  # 2 True [Point(x=0, y=5), Point(x=1, y=2)]
    print(f"seen = {seen}")               # seen = {Point(x=1, y=2), Point(x=0, y=5)}
```

（这个例子原样收在 `tests/run/traits_design_example.ty`，两个后端都跑。）

**trait 与实现**

1. `trait Name:` 声明一组方法签名：没有方法体的是必需方法，有方法体的是默认实现（类里可以覆盖）。签名里用 `Self` 指代实现类型，如 `fn __eq__(self, other: Self) -> bool`；类自己的方法里同样可以写 `Self`。trait 方法必须以 `self` 开头，trait 没有字段、不能带类型参数。**默认方法在 trait 定义处检查一次**：`Self` 是一个只拥有该 trait 方法的不透明类型；检查通过后，它被复制进每个声明了该 trait 却没有同名方法的类。
2. `class C(T1, T2):` 声明 `C` 实现 `T1`、`T2`；泛型类写作 `class Stack<T>(Sized):`。编译器检查每个必需方法都已定义且签名一致——把 `Self` 换成 `C` 之后，`self` 之外的参数类型逐个相同、返回类型相同（参数名与默认值不比较）；缺失或不匹配时报错并指出是哪个 trait 的哪个方法。括号里写类名报 `inheritance is not supported`，写未知名字报 `cannot find trait`。声明 `Ord` 或 `Hash` 必须同时声明 `Eq`。不加约束的泛型类在每个实例上检查一致性（模板语义，错误带实例化栈）。
3. **泛型约束与「检查型」泛型**：`fn f<T: Shape>(...)`、`class Box<T: Repr>`，多个约束写 `T: Eq + Hash`。规则按**整个条目**（一个泛型函数，或一个泛型类连同它的全部方法）划分：
   - 条目里**至少有一个类型参数带约束**，它就在**定义处检查一次**，此时**所有**类型参数都是不透明类型。带约束的 `T` 恰好支持约束提供的能力：约束 trait 的方法（方法调用语法），以及内建约束对应的运算符与内建函数（`Eq`：`==` `!=` `in`；`Ord`：`<` `<=` `>` `>=`、两参数的 `min` / `max`；`Hash`：`hash(x)`；`Repr`：`print`、f-string、`str`、`repr`；`Add` 等：对应运算符），`Ord` / `Hash` 蕴含 `Eq`。通过约束调用方法（含 trait 默认方法体里对 `self` 的调用）**只能按位置传参**：第 2 条不比较参数名，实现可以给参数起不同的名字，`int` 等内建类型的 dunder 则根本没有参数名，所以 `x.sub(a=10, b=1)` 在定义处就报错；具体类型上的调用照常使用该方法自己的参数名。同一条目里**不带约束的 `T`** 只能传参、赋值、返回、放进容器（`list<T>`、`T | None` 等）。对不透明 `T` 写 `is`、对裸 `T` 写 `is None` 都报错（`T` 可能是 `int`，没有身份）。条目内部调用的其他泛型按它们自己的约束检查（调用模板泛型时，用不透明的 `T` 检查其函数体）。这次检查产生的实例全部丢弃，不进入 codegen；条目随后照常单态化。检查出错的条目只在定义处报一次，之后对它的调用不再报错。
   - **一个约束都没有**的条目保持 3.9 的模板语义（单态化之后再检查），已有泛型代码不受影响。
   - 调用处与实例化处检查每个类型实参是否实现了对应约束：类必须**声明**了该 trait（只有 dunder 方法不算）；内建类型按下文的表格。两种写法都单态化，零运行时开销。
4. **只有静态分派**：trait 只能用作泛型约束，不能作为值的类型（`list<Shape>` 报 `the trait Shape cannot be used as a type`）。带虚表的 trait 对象等出现真实需求时再设计。
5. **M3 不做**：给已有类型补实现（`impl Shape for int` 式的 retroactive impl）、用户 trait 之间的继承（supertrait）、关联类型、泛型 trait（`trait Into<T>`）、泛型方法、反射运算符。

**运算符与内建 trait**

| 内建 trait | 方法 | 启用的语法与能力 |
|---|---|---|
| `Eq` | `__eq__(self, other: Self) -> bool` | `==`、`!=`（恒为 `not __eq__`；定义 `__ne__` 是错误）、`list` 的 `in`、容器的 `==` |
| `Ord` | `__lt__(self, other: Self) -> bool`，要求同时声明 `Eq` | `<`；`a > b` 即 `b < a`，`a <= b` 即 `not (b < a)`，`a >= b` 即 `not (a < b)`；两参数的 `min` / `max`；`sorted` |
| `Hash` | `__hash__(self) -> int`，要求同时声明 `Eq` | `hash(x)`；作为 `dict` 的 key、`set` 的元素（须声明，见下方「类作为 key」） |
| `Repr` | `__repr__(self) -> str` | `print(obj)`、f-string `{obj}`、`str(obj)`、`repr(obj)`、在容器里显示 |
| `Add` / `Sub` / `Mul` | `__add__` / `__sub__` / `__mul__(self, other: Self) -> Self` | `+` / `-` / `*` |
| `Div` / `FloorDiv` / `Mod` | `__truediv__` / `__floordiv__` / `__mod__(self, other: Self) -> Self` | `/` / `//` / `%` |
| `Neg` | `__neg__(self) -> Self` | 一元 `-x` |

- **在具体类型上，运算符只看 dunder 方法是否存在**（声明 trait 与否都行）：`a + b`（`a` 是用户类）直接编译成对 `a.__add__(b)` 的调用，`b` 按方法的参数类型检查，所以 `fn __mul__(self, k: float) -> Vec` 可以支持 `v * 2.0`、`fn __eq__(self, other: float) -> bool` 可以支持 `c == 40.0`。比较的改写保持操作数从左到右求值；比较链 `a < b < c` 照常只求值中间操作数一次。没有反射运算符：`2.0 * v` 是类型错误（不会去找 `__rmul__`）。在内建类型上以方法调用语法写 dunder，`(3).__lt__(4)`、`"a".__hash__()`，等同于对应的运算符 / 内建函数，这样带约束的泛型里 `x.__hash__()` 对每个满足约束的类型都成立。
- **容器里的类**：`print` / `==` / `in` 穿过 `list`、tuple、`dict` 的值与 `T | None` 到达元素时，使用元素类的 `__repr__` / `__eq__`；这要求方法是标准签名（`__eq__(self, other: C) -> bool`、`__repr__(self) -> str`），声明 trait 与否都行。没有 `__repr__` 的类 `print` 仍报 `cannot print`，没有 `__eq__` 的类写 `==` 仍报错并提示用 `is`。
- **派生**：类在括号里声明了 `Eq` / `Ord` / `Hash` / `Repr` 却没写对应方法时，编译器按字段逐个生成实现，要求每个字段的类型都**实现**该 trait（类字段须声明它），否则逐个字段报错：`Eq` 逐字段 `==`（没有字段时为 `True`）；`Ord` 与 tuple 相同的字典序，按字段声明顺序取第一个不相等的字段比 `<`；`Hash` 用 FNV 式的乘法混合组合各字段哈希（确定性，但不承诺跨版本稳定）；`Repr` 输出 dataclass 风格的 `Point(x=1, y=2)`，`str` 字段像在容器里一样带引号，泛型类实例用模板名（`Box(item=1)`）。派生出的方法是普通方法，也可以直接调用（`p.__eq__(q)`）。不声明就没有默认实现——这正是 M2 拒绝给 `print(obj)` / `obj == obj` 一个隐式默认值的原因，现在改为显式选择加入。
- **内建类型的实现**（约束检查用）：`int` 实现 `Eq`、`Ord`、`Hash`、`Repr`、`Add`、`Sub`、`Mul`、`FloorDiv`、`Mod`、`Neg`（不实现 `Div`：`int / int` 是 `float`，不是 `Self`）；`float` 实现 `Eq`、`Ord`、`Repr` 与全部六个算术 trait，不实现 `Hash`（NaN 无法满足哈希一致性）；`bool` 实现 `Eq`、`Ord`（`False < True`）、`Hash`、`Repr`；`str` 实现 `Eq`、`Ord`、`Hash`、`Repr`、`Add`；`tuple` 在所有元素都满足时实现 `Eq`、`Ord`（字典序）、`Hash`、`Repr`；`list` 在元素满足时实现 `Eq`、`Repr`（可变，不实现 `Hash`），并实现 `Add`（拼接）；`dict` / `set` 在元素满足时实现 `Repr`（它们还没有 `==`，见 3.10）；`T | None` 在 `T` 满足时实现 `Eq`、`Hash`、`Repr`。用户 trait 只能由声明它的类实现。
- `is` 仍然比较身份，与 `Eq` 无关。
- 新增内建函数 `repr(x)`（`x` 在容器里会被打印成的文本，作为 `str`）与 `hash(x)`（`int`；`int` 的哈希是它自己，`str` 与 dict 的 key 用同一个哈希函数）。

**类作为 key**（`dict` 的 key、`set` 的元素）：类必须**声明** `Hash`（于是也声明了 `Eq`），派生的与手写的都行；泛型类实例在字段满足时可以，检查型泛型里带 `T: Hash` 约束的 `T` 也可以（如 `class Counter<T: Hash>: counts: dict<T, int>`）。所有 dict / set 操作——`d[k]`、`d[k] = v`、`in`、`get`、`pop`、`add` / `remove` / `discard`、字面量与推导式——都可用，迭代、`keys()` / `values()` / `items()`、`sorted(d)` 与打印照常（打印用 key 类的 `__repr__`）。
- **不回调**：运行时从不调用用户代码（wasm 上运行时是另一个模块，没有共享函数表）。每个操作由生成代码先调用 key 的 `__hash__`，再调用 sema 为该类写的**探测函数**（HIR，对 codegen 只是普通函数）：它用 `ty_dict_probe(table, hash, after)` 逐个取出 64 位哈希相等的候选位置、用 `ty_dict_probe_key` 取出存着的 key，以 `stored.__eq__(key)` 确认（与 CPython 一样，存着的 key 是接收者），返回命中的位置或 -1；最后一次 `ty_dict_probe_slot` / `_insert` / `_remove` 把位置变成 value 槽位（未命中的 `d[k]` 由生成代码调用 `__repr__` 后 `ty_dict_key_missing` panic）。`__eq__` 是直接调用、可内联；探测状态在调用方的局部变量里，所以 `__eq__` 内对同一张表的查找互不干扰。`int` / `bool` / `str` / tuple key 的快速路径与生成代码完全不变。
- **哈希的混合**：运行时把 `__hash__` 的结果像 `int` key 一样再混合一次（`hash_int`）再取槽位，所以 `return self.x` 这样的弱哈希也分布均匀，只有 64 位哈希相等的条目才会调用 `__eq__`。
- **哈希一致性**：`a == b` 必须蕴含 `hash(a) == hash(b)`。派生的 `Hash` 与派生的 `Eq` 逐字段对应——每个字段的哈希都与该字段的 `==` 一致（`int` 是自身、`str` 按内容、类字段用它自己声明的 `Hash` / `Eq`）——所以两者都派生时自动一致；手写 `__hash__` 或 `__eq__` 时由程序员保证。违反这条规则、把 key 放进表之后修改它的字段、或在 `__eq__` 里修改正在查找的表，结果都可能找不到已有的 key 或插入重复的 key，但不会破坏内存安全：运行时在使用前重新校验生成代码交回的每个位置。

**`sorted` 作用于 `Ord` 类型**：运行时只排序标量及其扁平 tuple；其他实现 `Ord` 的元素类型（类、含类的 tuple、带 `T: Ord` 约束的 `T` 的实例）由 sema 按元素类型单态化出一份自底向上的归并排序（HIR，两个缓冲区交替），只调用 `__lt__`：右段元素严格小于左段元素时才先取右段，因此是稳定排序，与 Python 一样只依赖 `<`。

**容器的文本**：f-string 的 `{x}` 与 `str(x)` 接受容器（及其 `T | None`），文本就是 `repr(x)`，即 `print` 写出的内容；因此 `T: Repr` 的检查型泛型里写 `f"{x}"`，用容器类型实例化同样成立。渲染容器时调用的元素 `__repr__` 是用户代码：它自己的 `print` 照常写到标准输出，不会混进容器的文本（`repr(xs)`、f-string 与 `str(xs)` 返回的恰好是容器的文本）；`print(xs)` 先渲染出整段文本再写出，所以这些输出出现在容器之前，与 Python 一致。`__repr__` 里再对容器调用 `repr` 也照常嵌套。

**实现要点**：派生方法与少量辅助函数（tuple 的 `<` 与哈希、`T | None` 的哈希、类上的 `min` / `max`、类 key 的探测函数、`Ord` 类型的归并排序）由 sema 直接写成 HIR，对 codegen 而言只是普通函数。codegen 多认识的事：`hir::Program::protocols` 里登记的类 `__eq__` / `__repr__`（在容器元素的比较与打印循环里调用）与类 key 的 `__hash__` / 探测函数（每个表操作调用一次）、探测函数里的两个 HIR 节点 `TableProbe` / `TableKey`（各是一次运行时调用）、`repr(x)`（用运行时的输出捕获 `ty_repr_begin` / `ty_repr_end` 复用打印器；为元素调用 `__repr__` 时用 `ty_repr_suspend` / `ty_repr_resume` 暂停捕获）、`str` 与 `bool` 的哈希（`ty_str_hash`），以及 `bool` 的大小比较（无符号比较，`False < True`）。定义处检查的不透明类型参数是 sema 内部的伪类，检查结束后连同其实例整体回滚；类 key 的探测函数在所有函数体检查完之后，按类型表里出现过的 `dict<C, V>` / `set<C>` 统一登记。

---

## 4. 类型系统

### 4.1 内建类型

| 类型 | 表示 | 说明 |
|---|---|---|
| `int` | i64 | 默认整数类型，溢出回绕 |
| `float` | f64 | 默认浮点类型 |
| `bool` | i1（存储为 i8） | `True` / `False`，无 truthiness |
| `str` | 不可变 UTF-8 引用 | `{ptr, len}`，GC 堆上不可变 |
| `i8` `i16` `i32` `i64` | 定宽有符号整数 | 显式使用，无隐式提升 |
| `u8` `u16` `u32` `u64` | 定宽无符号整数 | 同上 |
| `f32` `f64` | 定宽浮点 | `float` 即 `f64` 的别名 |
| `list<T>` | 动态数组 | 引用类型，连续存储 |
| `dict<K, V>` | 哈希表 | 引用类型，插入有序的紧凑哈希表（runtime 的 `dict.rs`，见 3.10） |
| `set<T>` | 哈希集合 | 引用类型，同一张表，value 为零字节 |
| `tuple<A, B, ...>` | 定长异构元组 | **值类型**，按字段展开 |
| `None` | unit | 唯一值 `None`，零大小 |

### 4.2 值语义与引用语义

| 分类 | 类型 | 赋值/传参行为 |
|---|---|---|
| 值 | `int` `float` `bool`、定宽数值、`tuple` | 复制 |
| 不可变引用 | `str` | 共享指针，内容不可变，语义上等价于值 |
| 可变引用 | `class` 实例、`list` `dict` `set` | 共享指针，一处修改处处可见 |

这条划分和 Python 直觉一致（改 list 会影响调用方，改 int 不会），但**原因不同**：Python 是「一切皆对象 + 不可变性」，typhoon 是「小的定长数据按值，堆上容器按引用」。

`bool` 在寄存器与局部变量槽中是 `i1`，在 class 字段、list 元素与 tuple 成员中存储为 `i8`（存入 `zext`、取出 `trunc`）；这样容器元素有确定的字节大小，同时布尔运算仍在一位上完成。`tuple` 作为 LLVM 的一等聚合值（first-class aggregate）按值传参与返回，成员用 `insertvalue` / `extractvalue` 访问，不经过内存。

### 4.3 数值语义

- **整数溢出回绕**（Go 语义），不 panic。理由：内层循环里不插入溢出检查分支，是第 2 节性能目标的前提之一。*开放*：debug 模式下是否改为 trap（见第 9 节）。
- **无隐式数值转换**，包括 `int` → `float`。必须写 `float(x)` / `int(x)` / `i32(x)`。`int(f)` 向零截断。
- 混合宽度运算非法：`a: i32 + b: i64` 报错，需要显式转换。
- `/` 在两个整数上返回 `float`；`//` 与 `%` 采用 Python 的向下取整（floor）语义，商向负无穷取整、余数符号跟随除数。
- **除零 panic**（整数除零、`%` 零）；浮点除零遵循 IEEE 754，得到 `inf` / `nan`，不 panic。
- **索引越界 panic**，`dict` 取不存在的 key panic。panic 打印消息与位置后以非零码退出（MVP 不可捕获，M4 与异常体系合并考虑）。
- **list / str 的负索引采用 Python 语义**：`xs[-1]` 是最后一个元素，`xs[-len(xs)]` 是第一个；调整后仍越界则 panic（`list index out of range`）。边界检查内联为一次无符号比较（负索引同时被它挡下），负索引的 `+ len` 修正放在冷路径上，因此不使用负索引的循环不为该特性付出代价。
- **`len(str)` 按 Unicode 码点计数**（Python 语义），长度缓存在字符串头部 `{byte_len, char_len}` 中，因此是 O(1)，不需要扫描 UTF-8。
- **`for c in s` 产出长度为 1 的 `str`**（一个码点一个值）；ASCII 走运行时的 128 项静态字符串表，整个扫描不分配。
- **浮点运算允许 FMA 合并**（与 Go 一致）：`a * b + c` 可以被降低为一条 fused multiply-add，因此 `fadd` / `fsub` / `fmul` 带 `contract` 标志，其余快速数学标志一律不开。**显式 `float(x)` 转换是合并屏障**（同 Go 的显式转换规则），在 IR 上是一次 `llvm.arithmetic.fence.f64`：需要逐位可复现的浮点结果时，用它把要保留的中间量括起来。
- **幂运算的降低**：`x ** 0.5` 编译为 `sqrt`（硬件指令，不是 libm 的 `pow`），`x ** 2` / `x ** 2.0` 编译为 `x * x`；其余浮点幂走 `llvm.pow.f64`。整数幂由运行时的 `ty_int_pow` 按平方求幂计算，同样回绕；**负指数 panic**（`int ** int` 的结果必须是 `int`，需要负指数请写 `float(x) ** float(y)`）。

### 4.4 类型推断

- **函数签名必须完整标注**（参数与返回值），签名不参与推断。这保证了类型检查是模块内局部的，也保证错误信息指向声明处而不是遥远的调用点。
- **函数体内做双向局部类型推断**（bidirectional type checking）：自顶向下传播期望类型（checking 模式）、自底向上综合类型（synthesis 模式）。
- **不做全程序 Hindley–Milner**：没有全局约束求解、没有类型变量泛化。代价是空容器字面量需要标注（`xs: list<int> = []`），收益是推断结果可预测、错误信息定位准确、编译速度线性。
- 泛型函数调用点通过实参类型做**一阶匹配**推断类型实参，失败则要求显式实例化。

### 4.5 泛型的实现：单态化

每个 `(泛型函数, 类型实参元组)` 组合生成一份特化代码，符号名做 name mangling。好处是零抽象开销、可完全内联、数值代码可向量化；代价是编译时间与二进制体积随实例化数量增长（后续可加实例化缓存与代码去重）。

### 4.6 内建函数与输出格式

MVP 提供的内建：`print`、`len`、`range`、`float` / `int` / 定宽转换、`sorted`、`abs`、`min`、`max`、`str`；M3 加入 `repr` 与 `hash`（见 3.11）。

容器方法（MVP）：`list` 有 `append` / `pop` / `insert` / `clear`；`str` 有 `upper` / `lower` / `split(sep)` / `join(parts)` / `strip`（`str` 不可变，方法一律返回新串）；`dict` 有 `keys` / `values` / `items` / `get` / `pop`；`set` 有 `add` / `remove` / `discard`。

`print` 的输出格式（golden test 依赖它，必须稳定）：

| 类型 | 输出 |
|---|---|
| `int` | 十进制，负号前置 |
| `float` | 最短往返表示，且**始终带小数点**：`5.0`、`3.5`、`1e+30` |
| `bool` | `True` / `False` |
| `str` | 原文，不加引号 |
| `None` | `None` |
| 容器 | 类 Python 的 repr：`[1, 2]`、`{"a": 1}`、`{1, 2}`（空 set 为 `set()`）、`(1, "a")`，`str` 元素带引号；dict / set 按插入顺序输出 |

`print` 接受多个实参时以单个空格分隔，末尾换行。

---

## 5. 内存管理

### 5.1 MVP：Boehm GC

- 使用 **Boehm-Demers-Weiser GC（bdwgc）**，**静态链接**进产物，保持「单二进制」承诺。
- 所有堆对象经由 runtime 的 `ty_alloc(size, kind)` 分配；编译器不直接调用 `malloc`，也不生成裸指针算术之外的内存操作。
- 选 Boehm 的唯一理由是**它让我们跳过整个 GC 工程**，先把语言前端和 codegen 做对。它是保守式、stop-the-world 的标记清扫，性能不理想，但正确性成本极低。

### 5.2 演进路线

1. **逃逸分析（M5）**：在 IR 层判定对象是否逃出当前函数（被返回、被存入堆对象、被传给可能存储它的调用），不逃逸的直接栈分配。这是分配密集基准上收益最大的一步。
2. **替换 Boehm（M5 之后评估）**，二选一：
   - **RC + move 语义（Nim ORC 模型）**：编译期插入引用计数增减，配合 move 分析消除大部分计数操作，循环引用由 cycle collector 兜底。优点是延迟可预测、内存及时回收、对 FFI 友好；缺点是编译器复杂度高。
   - **精确分代 GC**：需要精确的栈图（stack map）与写屏障，吞吐更好，但与未来并发模型耦合更深。
3. 决策点放在 M5，前置条件是并发模型（第 9 节）已经明朗——可增长栈、并发标记与保守扫描互相冲突，必须一起定。

### 5.3 明确不做

**不引入 Rust 式的所有权 / 借用 / 生命周期语法。** typhoon 的目标用户是「写 Python 但要性能」的人，生命周期标注会直接摧毁学习曲线。内存安全靠 GC + 边界检查保证，而不是靠类型系统。

---

## 6. 编译器架构

### 6.1 Crate 划分（Rust workspace）

| crate | 职责 |
|---|---|
| `diag` | `Span`（file_id + 字节区间）、`Diagnostic`（message、主标签、次标签、note、help）、渲染器；lexer/parser/sema 都依赖它。渲染用 `annotate-snippets`（rust-lang 维护，cargo 已用、rustc 正在迁移）或 `ariadne` |
| `lexer` | 手写或 logos；缩进感知，产出 INDENT/DEDENT/NEWLINE；每个 token 带 span |
| `parser` | 手写递归下降 + Pratt 表达式解析；带错误恢复（同步到行首/DEDENT）；泛型 `<>` 的推测解析与 `>>` 拆分 |
| `ast` | 纯数据结构，无逻辑；节点带 span |
| `sema` | 名字解析、类型推断与检查、定值分析、单态化；类型经 interning 表示为 `TypeId` |
| `ir` | 带类型的中间表示（CFG + 简单 SSA），后端无关；逃逸分析等优化在此层。**推迟到 M3**（排在逃逸分析之前）：M0–M2 没有独立的 IR crate，`codegen` 直接下降 `sema` 输出的 typed HIR |
| `codegen` | IR → LLVM IR（M0–M2 是 typed HIR → LLVM IR） |
| `codegen-wasm` | typed HIR → WebAssembly 二进制（`wasm-encoder` 直出），与 `codegen` 语义对齐；用于浏览器执行与 `typhoon build --target wasm` |
| `runtime` | Rust `staticlib`，`extern "C"` 导出 `ty_alloc`、str/list/dict 操作、`ty_print_*`、`ty_panic` |
| `cli` | `typhoon build <file>`、`typhoon run <file>`、`typhoon emit-ir <file>` |

**手写 lexer/parser 而不是用 parser generator**：缩进敏感语法（INDENT/DEDENT、括号内隐式续行）本来就需要有状态的手写词法器；错误恢复与优质诊断也只有手写才能控制。这也意味着放弃当前仓库里基于 nom 的解析实现。

**为什么不用 nom 做 parser**：(a) rustc 风格的带上下文报错来自 `Span` + `Diagnostic` + 渲染器这三件套，与用什么解析技术无关——rustc、Go、Swift、Zig、ruff 的 parser 全都是手写的；(b) nom 用在 parser 层有三个硬伤：`alt` 的回溯会放大工作量（现有仓库里 `expression = alt((sum, call))` 最坏会把 `call` 解析三次）、错误信息只有字节偏移量和组合子名字（说不出「这里期待 `:`，因为第 3 行的 `if` 开了一个块」）、以及没有错误恢复，一次编译只能报一个错；(c) parser 性能根本不是编译速度的瓶颈，`clang -O2` 占绝对大头，何况在 token 流上手写递归下降通常比 nom 更快（ruff 从 lalrpop 换成手写 parser 后解析速度约翻倍）；(d) lexer 层则可以接受生成器：`logos`（DFA 生成器）或手写都行；nom 若一定要保留，只应该用于 lexer 的 token 识别，缩进栈与 INDENT/DEDENT 的合成仍然必须手写。

`sema` 输出 typed AST，`ir` 从 typed AST 下降。单态化发生在 sema 末尾（此时类型信息完整），IR 里已经没有泛型。

### 6.2 后端策略（已决定：不用 inkwell）

**阶段一（M0–M4）：生成文本 LLVM IR。**

`codegen` 直接输出 `.ll` 文本，然后调用本机 clang 完成优化与链接：

```sh
clang -O2 out.ll libtyphoon_runtime.a -lgc -o out
```

理由：

- **零 FFI**：不链接 libLLVM，没有 unsafe，没有 llvm-sys 的版本绑定与构建脚本地狱。
- **CI 简单**：只需 `brew install llvm` / `apt install clang`。
- **代码量最小**：IR 打印器就是字符串拼接，可读、可 diff、可直接进 golden test。
- **优化效果与二进制 LLVM 完全一致**：优化由真正的 `-O2` 管线完成，我们不重复造轮子。

以 **LLVM 18+ 的 IR 语法**为准（opaque pointers，即统一的 `ptr` 而非 `i8*`）。

**阶段二（M5，需要 JIT 支撑 `typhoon run` 与 REPL 时）：** 自己写一个薄的 `llvm-sys` FFI 封装 crate，只封装 IR 构建 + ORC JIT 所需的最小子集（不做通用安全封装）。`codegen` 通过 trait 抽象输出目标，同时支持「文本 .ll」与「内存中 LLVM 模块」两条路径。

**备选方案**：如果 LLVM 依赖在 CI 上长期麻烦，退回 **C 后端**（Nim 路线），生成 C 再调 cc。保留这个选项，但不作为主路线。

#### 6.2.1 wasm 后端

浏览器执行与 `typhoon build --target wasm` 走一条独立于 LLVM 的下降路径，由 `codegen-wasm` 承担。以下五条已定稿：

1. **runtime 单一实现**：`crates/runtime` 只有一份源码，既编译为原生 `staticlib`，也用 `cargo rustc --crate-type cdylib --target wasm32-unknown-unknown` 编译成一个 wasm 模块，导出同一套 `ty_*` C ABI 与线性内存 `memory`。wasm 下只有三处不同：分配走 Rust 全局分配器且永不回收（v1 无 GC）；stdout / stderr 经宿主导入函数 `host_write(fd, ptr, len)` 输出；`ty_rt_exit` / `ty_panic` 调用宿主导入的 `host_exit(code)` 后以 `unreachable` 陷入。原生行为不变。
2. **双模块共享内存**：用户程序是独立的第二个 wasm 模块，`import` runtime 实例的 `env.memory` 与用到的每个 `env.ty_*`。加载器先实例化 runtime（提供 `host_write` / `host_exit`），再用它的导出实例化用户模块，然后调用用户模块导出的 `_start`。加载器 JS 只有一份（`crates/driver/assets/loader.js`），CLI 内嵌它，playground 也用它。
3. **`wasm-encoder` 直出**：不经过 LLVM、也不经过文本 WAT，`codegen-wasm` 把 typed HIR 直接下降成二进制模块——HIR 的结构化控制流与 wasm 的 `block` / `loop` / `if` / `br` 一一对应。指针是 i32，因此布局是 target-aware 的：指针 4 字节，`str` / `list` 的头部字段顺序与原生一致，只有指针宽度不同；`int` = i64、`float` = f64、`bool` 在局部变量里是 i32、在内存里是 i8。
4. **字符串字面量用被动数据段**：用户字符串字面量放进 passive data segment，`_start` 里先 `ty_alloc_atomic` 分配、写入 `{byte_len, char_len}` 头部，再用 `memory.init` 拷贝字节，指针存进 wasm global。这样字面量与运行时分配共用同一个堆，不需要为静态数据预留地址空间。
5. **差分测试**：`tests/run/*.ty` 与 `examples/*.ty` 的每个用例都在 wasmtime 下再跑一遍 wasm 后端，stdout 与 `# expect:` 逐字节比对，`# exit:` / `# stderr:` 同样生效，测试名为 `wasm/run/<name>`。需要 GC 才能跑的用例用 `# wasm: skip <reason>` 显式跳过（v1 wasm 后端不回收内存）。此外每个产出的模块都用 `wasmparser` 校验，并对约 10 个小程序做 `wasmprinter` 的 WAT 快照。

wasm 输出的性能不是 v1 的目标——第 2 节的基准比值一律只针对原生后端——正确性对齐才是：同一个程序在 wasmtime 与原生下必须给出逐字节相同的输出。

### 6.3 编译管线

```
source (.ty)
  → lexer      (tokens + INDENT/DEDENT/NEWLINE, spans)
  → parser     (AST)
  → sema       (name resolution → type inference/check → definite assignment → monomorphization ⇒ typed AST)
  → ir lowering(typed CFG / SSA, escape analysis @M5)          （M3 起）
  → codegen    (text LLVM IR)
  → clang -O2  (optimize + link with runtime & GC)
  → native binary
```

wasm 后端（6.2.1）从 typed AST 处分叉，共用整个前端：

```
typed AST
  → codegen-wasm (binary wasm module，wasm-encoder 直出)
  → <name>.wasm + typhoon_rt.wasm + loader.js
  → node / 浏览器 / wasmtime
```

`typhoon emit-ir` 在 `.ll` 这一步停下，用于调试与 golden test。

---

## 7. 测试与基准

### 7.1 Golden tests

- 成功用例放 `tests/run/*.ty`，在文件内用 `# expect: <line>` 注释声明期望的 stdout，**逐行比对**（顺序、数量都要一致）。
- 失败用例放 `tests/fail/*.ty`，用 `# error: <substring>` 声明期望的诊断信息子串（子串匹配，避免绑定完整措辞）。
- 测试驱动统一编译并运行每个文件，比对实际输出。`examples/` 下的示例同样带 `# expect:` 注释，纳入同一套驱动，保证文档里的例子永远是可运行且正确的。

```python
# examples/hello.ty
# expect: Hello, Typhoon!

fn main():
    print("Hello, Typhoon!")
```

### 7.2 基准

- 每个基准一个目录：`bench/<name>/main.ty` 与同目录下等价的 `main.go`。两份实现必须算法一致（同样的数据结构、同样的循环结构），否则比值没有意义。
- `bench/run.sh` 各跑 N 次取**中位数**，输出 typhoon 相对 Go 的耗时比值表。
- **从 M1 开始每个 PR 都跑基准**，比值回退超过阈值时 CI 标红。性能是可回归的功能，不是一次性冲刺。

---

## 8. 路线图

| 里程碑 | 内容 | 退出标准 |
|---|---|---|
| M0 重置 | 删除旧 crate 与注释掉的代码；新 workspace 骨架；安装工具链：`brew install llvm bdw-gc go`（Go 用于 bench 对照，当前开发机未安装 Go 与 bdw-gc）/ CI；runtime 骨架 + Boehm 链接；golden test 驱动 | `hello.ty` 编译运行通过 CI |
| M1 数值核心 | lexer/parser、`fn`、`int`/`float`/`bool`、运算符、`if`/`while`/`for range`、递归、`print`、类型检查 | fib / nbody / mandelbrot / spectral-norm ≤ 1.0x Go |
| M2 数据 | `class`、`str`、`list<T>`、`tuple`、`for-in`、Boehm 接入 | binary-trees ≤ 2.0x、fannkuch ≤ 1.0x |
| W1 wasm 后端 | HIR → wasm、共享 runtime、CLI `--target wasm`、playground 在线执行 | 全部 golden 用例在 wasmtime 下与原生输出一致 |
| M3 泛型与推断 | 泛型单态化、`dict`/`set`、推导式、`T \| None`、比较链、名义 trait 与运算符重载 | k-nucleotide ≤ 1.5x |
| M4 错误与模块 | `try`/`except`/`raise`、`import`、C FFI、字符串运行时优化 | fasta / k-nucleotide ≤ 1.0x |
| M5 性能与体验 | 逃逸分析、GC 替换评估、JIT `run`、诊断（ariadne / miette 风格）、formatter | binary-trees ≤ 1.0x |
| M6 并发 | 模型待定（见开放问题） | 待定 |

每个里程碑的退出标准都是**可测量的**：要么是 CI 上跑通的用例，要么是基准比值。没达到就不进下一阶段。

---

## 9. 开放问题

以下问题尚未决定，标注了希望决策的时点。

1. ~~**继承 vs traits/protocols**~~ **已决定（2026-10-12）**：名义 trait，不做继承，只有静态分派，见 3.11。仍然开放的子问题：带虚表的 trait 对象、retroactive impl、supertrait。
2. **异常 vs Result**：`try/except/raise` 是 Python 观感的核心，但零成本异常实现复杂、且与「错误必须显式处理」的静态语言直觉冲突。可能的折中：底层用 `Result` + `?` 风格传播，语法层保留 `raise`/`except` 皮肤。**M4 前必须决定。**
3. **并发模型**：goroutine + channel（需要可增长栈、调度器，与保守 GC 冲突）？async/await（需要状态机变换与运行时）？还是只提供 OS 线程 + 通道？该选择直接约束 GC 与栈设计，**必须与 M5 的 GC 决策一起做**。
4. ~~**运算符重载**~~ **已决定（2026-10-12）**：允许，dunder 方法、静态分派，见 3.11。反射运算符（`__radd__`）尚未决定。
5. **`str` 的索引语义**（**部分决定，M3**）：`s[i]` 是字节、码点还是字素簇？Rust 选择禁止整数索引，Go 返回字节，Python 返回码点。UTF-8 存储下按码点索引是 O(n)，与性能目标冲突。倾向禁止整数索引、只提供切片与迭代器。**已决定的部分——切片 `s[a:b]`**：偏移以**码点**为单位，与 `len(s)`、`s.find(sub)` 的单位一致（`s[s.find(x):]` 即字面意思）；沿用 Python 语义：负偏移从末尾数，越界偏移截断到 `[0, len]` 而不是报错，省略的界即开头 / 结尾，空区间得到空串；不支持步长（`s[a:b:c]` 报错），list 切片暂不支持。码点偏移天然落在 UTF-8 边界上，不会切开字符。代价：纯 ASCII 串（`byte_len == char_len`）是 O(b − a) 的拷贝，非 ASCII 串需要一次 O(n) 扫描；单个 ASCII 字符的切片取自 runtime 的静态表，整串切片返回原串，两者都不分配。**仍未决定**：`s[i]` 本身——M3 依旧报错并提示改用迭代、切片或 `find` / `split`。
6. **debug 模式整数溢出 trap**：release 回绕已定；debug 下是否插入溢出检查并 panic（Rust 模型）？
7. **`match` / `case`**：是否引入模式匹配，以及是否与 `T | None`、未来的 enum/sum type 统一设计。
8. **Python 互操作**：是否永远不做？还是将来提供一个「用 CPython 作为外部进程/嵌入解释器」的桥接层，以便复用 numpy 之类的生态？目前立场是不做，但这是最容易被市场压力推翻的一条。
