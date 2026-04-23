# Représentation intermédiaire

La RI est le pivot de toute la chaîne d'outils. Elle se situe entre l'élévateur (qui la produit) et les moteurs de génération de code (qui la consomment). Elle est entièrement définie dans `rustdec-ir`.

---

## Objectifs

- **Indépendante de l'architecture** — aucun registre après élévation, aucun opcode propre à x86.
- **Indépendante de la cible** — aucune syntaxe C/Rust intégrée ; les moteurs décident de la mise en forme.
- **Types explicites sur chaque valeur** — chaque variable SSA et chaque constante porte un `IrType`.
- **Annotée par niveau de confiance** — les blocs de base portent un score de confiance pour les déductions incertaines.

---

## Système de types

```rust
pub enum IrType {
    UInt(u8),                        // entier non signé, largeur 8/16/32/64 bits
    SInt(u8),                        // entier signé
    Float(u8),                       // flottant IEEE 32 ou 64 bits
    Ptr(Box<IrType>),                // pointeur typé (cible 64 bits supposée)
    Array { elem: Box<IrType>, len: u64 },
    Struct { name: String, size: u64 }, // structure opaque (nom tiré du DWARF ou synthétique)
    Void,
    Unknown,
}

pub type IrTypeRef = Arc<IrType>;   // nœud de type partagé par comptage de références
```

`IrTypeRef` est utilisé partout où un type apparaît. Le même nœud `Arc` est réutilisé pour toutes les variables du même type dans une fonction, évitant les allocations répétées.

Les types courants sont construits à l'aide de fonctions auxiliaires :
```rust
IrType::u64()   // UInt(64)
IrType::u32()   // UInt(32)
IrType::u8()    // UInt(8)
IrType::ptr()   // Ptr(UInt(8))  — équivalent void*
```

---

## Valeurs

```rust
pub enum Value {
    Var { id: u32, ty: IrTypeRef },       // variable SSA
    Const { val: u64, ty: IrTypeRef },    // constante entière / pointeur
}
```

Les variables sont identifiées par un `id` de type `u32`. Chaque variable est affectée exactement une fois (invariant SSA). `IrFunction` attribue de nouveaux identifiants via `fresh_var()`.

---

## Expressions

```rust
pub enum Expr {
    Value(Value),                          // copie / source phi
    BinOp { op: BinOp, lhs: Value, rhs: Value },
    Load { ptr: Value, ty: IrTypeRef },    // déréférencement mémoire
    Call { target: CallTarget, args: Vec<Value>, ret_ty: IrTypeRef },
    Cast { val: Value, to: IrTypeRef },    // transtypage / troncature / extension
    Symbol { addr: u64, kind: SymbolKind, name: Arc<str> },
    ArrayAccess { name: String, index: Value, elem_ty: IrTypeRef },
    Opaque(String),                        // expression non résolue / non élevée
}
```

`Arc<str>` est utilisé pour `Symbol::name` et `CallTarget::Named` afin que le même littéral de chaîne soit partagé entre tous les usages dans le module.

### Opérateurs binaires

| Variante | Signification |
|---|---|
| `Add`, `Sub`, `Mul` | arithmétique |
| `UDiv`, `SDiv` | division non signée / signée |
| `URem`, `SRem` | reste non signé / signé |
| `And`, `Or`, `Xor` | opérations sur les bits |
| `Shl`, `LShr`, `AShr` | décalages |
| `Eq`, `Ne` | comparaison d'égalité |
| `Ult`, `Ule` | inférieur / inférieur ou égal non signé |
| `Slt`, `Sle` | inférieur / inférieur ou égal signé |

### Cibles d'appel

```rust
pub enum CallTarget {
    Direct(u64),        // adresse statiquement connue
    Indirect(Value),    // cible calculée (pointeur de fonction)
    Named(Arc<str>),    // symbole importé (p. ex. "printf")
}
```

### Types de symboles

```rust
pub enum SymbolKind {
    String,     // pointe vers un littéral de chaîne dans .rodata
    Function,   // cible d'appel résolue vers une fonction connue
    Global,     // référence à une variable globale
}
```

---

## Instructions

```rust
pub enum Stmt {
    Assign { lhs: u32, ty: IrTypeRef, rhs: Expr }, // affectation SSA  %lhs = rhs
    Store { ptr: Value, val: Value },               // *ptr = val
    ArrayStore { name: String, index: Value, val: Value }, // arr[i] = val
    Nop,                                            // marqueur (code mort)
}
```

L'élimination du code mort remplace les affectations inutilisées par `Nop` plutôt que de les supprimer, préservant ainsi les indices d'instructions pour les passes ultérieures.

---

## Terminateurs

Chaque bloc de base se termine par exactement un terminateur :

```rust
pub enum Terminator {
    Jump(BlockId),                                       // saut inconditionnel
    Branch { cond: Value, true_bb: BlockId,
             false_bb: BlockId, mnemonic: String },      // branchement conditionnel
    Return(Option<Value>),                               // retour de fonction
    Unreachable,                                         // après ud2 / hlt
}
```

Le champ `mnemonic` de `Branch` préserve le code de condition x86 d'origine (p. ex. `"jne"`, `"jl"`) afin que les moteurs de génération puissent émettre des comparaisons idiomatiques.

---

## Blocs de base

```rust
pub struct BasicBlock {
    pub id: BlockId,                 // u32
    pub start_addr: u64,
    pub end_addr: u64,
    pub stmts: Vec<Stmt>,
    pub terminator: Terminator,
    pub confidence: f32,             // 0,0 – 1,0 ; < 1,0 indique une élévation incertaine
}
```

---

## Cadre de pile

L'analyse de cadre remplit `IrFunction::slot_table` avec un `StackSlot` pour chaque emplacement de pile adressé :

```rust
pub struct StackSlot {
    pub rbp_offset: i64,        // décalage signé par rapport à RBP
    pub ty: IrTypeRef,
    pub name: String,           // p. ex. "local_0", "arg_1", "saved_rbx"
    pub origin: SlotOrigin,
    pub array_info: Option<ArrayInfo>,
    pub provenance: Provenance,
}

pub enum SlotOrigin {
    Local,       // variable locale
    StackArg,    // argument passé sur la pile (au-delà de la fenêtre de registres)
    SavedReg,    // déversement de registre callee-saved
    Unknown,
}

pub struct ArrayInfo {
    pub count: u32,   // nombre d'éléments
    pub stride: u32,  // taille d'un élément en octets
}
```

`Provenance` enregistre la façon dont le type a été déterminé :

| Variante | Source |
|---|---|
| `Auto` | valeur par défaut synthétique |
| `Inferred` | déduit à partir des patterns d'utilisation |
| `Dwarf` | lu dans les informations de débogage DWARF |
| `User` | défini de façon interactive (à venir) |

---

## Fonctions et modules

```rust
pub struct IrFunction {
    pub name: String,
    pub entry_addr: u64,
    pub end_addr: u64,
    pub cfg: CfgGraph,              // petgraph DiGraph<BasicBlock, CfgEdge>
    pub params: Vec<IrTypeRef>,
    pub param_names: Vec<String>,
    pub ret_ty: IrTypeRef,
    pub next_var_id: u32,
    pub slot_table: Vec<StackSlot>,
    pub frame_size: u64,
    pub reg_names: HashMap<u32, String>, // var_id → nom du registre (débogage)
}

pub struct IrModule {
    pub functions: Vec<IrFunction>,
    pub string_table: HashMap<u64, String>, // adresse → contenu de la chaîne
}
```

`IrFunction::blocks_sorted()` retourne les blocs de base triés par adresse de début, ce qui correspond à l'ordre d'itération canonique utilisé par les moteurs de génération de code.

---

## RI structurée

Après structuration du GFC (dans `rustdec-analysis`), une `IrFunction` peut être convertie en arbre de `SNode` :

```rust
pub enum SNode {
    Block(BlockId),
    Seq(Vec<SNode>),
    IfElse { cond: CondExpr, then: Box<SNode>, else_: Option<Box<SNode>> },
    Loop { cond: Option<CondExpr>, body: Box<SNode> },
    Break,
    Continue,
}
```

Les moteurs de génération de code consomment les arbres `SNode` pour émettre une sortie structurée (`if`, `while`, `for`) sans `goto`.
