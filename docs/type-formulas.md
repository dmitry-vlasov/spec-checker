# Type Formula DSL

Abstract type constraints expressed as formulas, not structural duplication:

```yaml
exposes:
  CheckResult:
    kind: type
    type_constraints:
      - "is_product(Self)"
      - "has_field(Self, errors)"
      - "equals(con(field(errors)), Vec)"
      - "cloneable"

  SpecChecker.check:
    kind: function
    type_constraints:
      - "equals(param(spec), ModuleSpec)"
      - "fallible(return)"
      - "equals(con(return), Result)"
```

## Type Expressions

| Expression | Meaning |
|-----------|---------|
| `Self` | The type being specified |
| `field(name)` | Type of a struct field |
| `param(n)` or `param(name)` | Function parameter type |
| `return` | Function return type |
| `con(T)` | Type constructor (strip args): `con(Vec<String>)` = `Vec` |
| `arg(T, n)` | n-th type argument: `arg(Result<X, E>, 0)` = `X` |
| `apply(F, A, ...)` | Apply constructor to args |
| `function(A, B, ..., R)` | Function type (last = return) |
| `domain(F)` / `codomain(F)` | Parameter/return of function type |
| `sum(A, B, ...)` / `product(A, B, ...)` | Algebraic types |

## Predicates

| Predicate | Meaning |
|----------|---------|
| `equals(T1, T2)` | Type equality (strips references automatically) |
| `matches(T, pattern)` | Pattern match with `_` wildcards |
| `is_subtype(T, Super)` | Subtyping (trait impl, interface) |
| `is_sum` / `is_product` / `is_function` | Kind checks |
| `has_field(T, name)` | Struct has field |
| `has_method(T, name)` | Type has method |
| `has_variant(T, name)` | Enum has variant |
| `cloneable` / `serializable` / `send` / `sync` | Property checks |
| `fallible` | Returns Result or Option |
| `and` / `or` / `not` / `implies` | Logical connectives |

Type comparisons are **language-agnostic**: `equals` strips references (`&`, `&mut`) so specs express type dependencies, not language-specific passing conventions.
