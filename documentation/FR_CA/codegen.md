# Génération de code

`rustdec-codegen` consomme un `IrModule` et émet du pseudo-code dans le langage demandé.

---

## Point d'entrée

```rust
pub fn emit_module(module: &IrModule, lang: Language) -> CodegenResult<Vec<(String, String)>>
```

Retourne une paire `(nom_fonction, code_source)` par fonction. Les fonctions présentes dans la liste de filtrage CRT sont silencieusement ignorées (voir ci-dessous).

```rust
pub enum Language { C, Cpp, Rust }
```

---

## Trait commun

Tous les moteurs implémentent :

```rust
pub trait CodegenBackend {
    fn emit_function(&self, func: &IrFunction, string_table: &HashMap<u64, String>) -> String;
    fn emit_type(&self, ty: &IrType) -> String;
}
```

---

## Moteur C

Le moteur C cible C99 avec les types standard de `<stdint.h>`.

### Correspondance des types

| `IrType` | Type C |
|---|---|
| `UInt(8)` | `uint8_t` |
| `UInt(16)` | `uint16_t` |
| `UInt(32)` | `uint32_t` |
| `UInt(64)` | `uint64_t` |
| `SInt(8)` | `int8_t` |
| `SInt(16)` | `int16_t` |
| `SInt(32)` | `int32_t` |
| `SInt(64)` | `int64_t` |
| `Float(32)` | `float` |
| `Float(64)` | `double` |
| `Ptr(T)` | `T*` |
| `Void` | `void` |
| `Unknown` | `uint64_t` |

### Passes d'émission

**Passe 1 — collecte des variables.**  
Parcourt tous les nœuds `Stmt::Assign`. Construit une table de copies (affectations directes variable à variable pouvant être propagées en ligne). Suit les variables effectivement écrites.

**Passe 2 — déclarations.**  
Émet une ligne `type nom;` par variable active en tête du corps de la fonction.

**Passe 3 — corps.**  
Parcourt l'arbre `SNode` structuré (issu de la structuration du GFC) et émet :
- `Seq` → instructions dans l'ordre.
- `IfElse` → `if (cond) { … } else { … }`.
- `Loop` → `while (cond) { … }`.
- `Break` / `Continue` → mots-clés seuls.
- `Block` → les instructions du bloc.

**Passe 4 — corrections de pointeurs.**  
Insère des transtypages lorsqu'un type pointeur est passé à une fonction attendant un type pointeur différent.

### Substitution de signatures libc

`libc_signatures.rs` contient une table de consultation pour les fonctions courantes de la bibliothèque C. Lorsqu'un `CallTarget::Named` correspond à une fonction connue (p. ex. `printf`, `strlen`, `malloc`), le moteur utilise la signature connue plutôt que la signature inférée. Cela produit :

```c
// sans substitution :   uint64_t v0 = printf(arg_0, arg_1);
// avec substitution :   printf("%s\n", local_0);
```

---

## Moteur C++

Le moteur C++ suit la même structure en quatre passes que le moteur C, avec ces différences :

- Les types utilisent `uint64_t` / les conventions C++ selon le contexte.
- `nullptr` à la place de `0` pour les pointeurs nuls.
- L'accès aux membres utilise `->` ou `.` selon le type pointeur.

---

## Moteur Rust

Le moteur Rust émet du pseudo-code Rust `unsafe` :

- Variables déclarées avec `let mut`.
- Types : `u64`, `u32`, `i32`, `*mut u8`, etc.
- Chargements mémoire rendus comme `*ptr`.
- Corps des fonctions enveloppés dans `unsafe { … }`.

La sortie est un pseudo-code seulement — il ne compilera pas tel quel, car les noms de variables SSA et les opérations sur les pointeurs bruts nécessitent un affinement supplémentaire.

---

## Filtrage CRT

Les noms de fonctions suivants sont exclus de la sortie quel que soit le langage, car il s'agit de code de protocole CRT sans valeur pour la décompilation :

`_start`, `_init`, `_fini`, `__libc_csu_init`, `__libc_csu_fini`, `__libc_start_main`, `frame_dummy`, `register_tm_clones`, `deregister_tm_clones`, `__do_global_dtors_aux`

---

## Table des chaînes

La `IrModule::string_table` (adresse → contenu, extraite de `.rodata`) est transmise à chaque appel d'émission. Lorsqu'un `Expr::Symbol { kind: SymbolKind::String, addr, name }` est rencontré, le moteur émet le contenu de la chaîne sous forme de littéral de chaîne C plutôt qu'une adresse brute :

```c
// brut :     printf((uint8_t*)0x402010, local_0);
// résolu :   printf("Bonjour, %s!\n", local_0);
```
