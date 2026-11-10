# Your First Program

Creating and running a program in Conduit requires only a single `.cdt` file.

---

## 1. Writing the Code

Create a file named `hello.cdt`:

```conduit
{{#include ../../../examples/docs/getting_started/hello.cdt}}
```

### Key Elements:
- `func main()`: The entry point of an executable Conduit program. Every root module intended for execution must define a `main()` function.
- `print(...)`: A built-in function that formats and prints any supported value to standard output, followed by a newline.

---

## 2. Type-Checking the Program

Before running any program, you can perform static validation with the `conduit check` command:

```bash
conduit check hello.cdt
```

Output:

```text
Check passed: 'hello.cdt' is valid.
```

The type checker analyzes the entire program (including any imported modules), verifies type safety, exhaustiveness of pattern matches, error handling, and private symbol access boundaries without executing the code.

---

## 3. Running the Program

Execute the program with `conduit run`:

```bash
conduit run hello.cdt
```

Output:

```text
Hello, Conduit!
```

`conduit run` runs the static type checker first. If any compile-time or type errors are discovered, execution halts immediately with precise source coordinates (`[line:col]`) and descriptive diagnostic messages.

