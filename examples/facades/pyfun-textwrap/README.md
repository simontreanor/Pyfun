# pyfun-textwrap: an example façade package

A façade is an ordinary pip package that ships typed `extern` declarations as `.pyfun` files. This
one wraps Python's `textwrap`, so it needs nothing but the standard library.

```bash
cd my-project
pyfun add path/to/Pyfun/examples/facades/pyfun-textwrap
```

`pyfun add` installs the package, copies `wrap.pyfun` into `.pyfun/facades/pyfun-textwrap/`, and
records the dependency in `pyfun.toml`. The module is then importable like one of your own:

```pyfun
import Wrap

"Pyfun façades are ordinary pip packages that carry typed declarations."
|> Wrap.fill 24
|> print
```
