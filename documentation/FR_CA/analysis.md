# Analyse

`rustdec-analysis` orchestre le pipeline complet et fournit plusieurs passes d'analyse statique opérant sur la RI brute.

---

## Point d'entrée principal

```rust
pub fn analyse(obj: &BinaryObject) -> AnalysisResult<IrModule>
```

Ordre d'exécution interne :

```
rayon::join(
  désassembler toutes les sections,
  extract_strings(obj)
)
build_symbol_map(obj, &strings)
detect_functions(obj, &instructions)
compute_function_boundaries()
rayon::par_iter: pour chaque fonction {
  build_cfg(nom, entrée, fin, insns)
  lift_function(func, insns, symbols)
}
build_call_graph(&module)
retourner IrModule
```

---

## Détection des fonctions

`detect_functions` effectue un balayage en 4 étapes et retourne un `BTreeMap<u64, String>` (adresse → nom) :

**Étape 1 — point d'entrée.**  
Le champ `e_entry` du binaire est toujours enregistré sous le nom `_start`.

**Étape 2 — table des symboles.**  
Tous les symboles de type `SymbolKind::Function` sont ajoutés. Les talons d'import de la PLT sont inclus.

**Étape 3 — balayage des sites d'appel.**  
Chaque instruction `call rel32` ajoute sa cible à la table si elle n'y figure pas déjà. Des noms synthétiques sont générés : `sub_<adresse>`.

**Étape 4 — tables de sauts indirects.**  
Les séquences de type talons PLT et les patterns de tables de saut de type `switch` sont détectés et leurs cibles enregistrées.

---

## Construction du GFC

`build_cfg` utilise un algorithme en 3 passes pour produire un GFC correct pour une seule fonction.

**Passe 1 — identification des leaders.**  
Un leader de bloc est toute instruction qui est :
- le point d'entrée de la fonction,
- une cible directe de branchement,
- l'instruction qui suit immédiatement un terminateur.

**Passe 2 — création des blocs.**  
Les instructions sont partitionnées en blocs de base aux frontières des leaders. Chaque bloc enregistre ses adresses de début et de fin.

**Passe 3 — connexion des arcs.**  
Pour chaque terminateur :
- `ret` → aucun arc sortant (le bloc est un puits).
- `jmp cible` → un arc vers le bloc cible.
- `jcc cible` → deux arcs : continuation (faux) et cible (vrai).
- `call` → traité comme non-terminateur ; l'exécution continue à l'instruction suivante.
- `jmp` indirect (p. ex. switch) → arcs vers toutes les cibles connues par balayage de table.

Le résultat est une `IrFunction` avec un `CfgGraph` peuplé mais des listes d'instructions vides.

---

## Analyse de dominance

`dominance::compute(func)` exécute l'algorithme de dominateurs de Cooper-Harvey-Kennedy sur le GFC et retourne un `DomTree`.

```rust
pub struct DomTree { … }

impl DomTree {
    pub fn idom(&self, node: BlockId) -> Option<BlockId>
    pub fn strictly_dominates(&self, a: BlockId, b: BlockId) -> bool
}
```

`find_natural_loops(func)` s'appuie sur l'arbre de dominance : tout arc retour `(n → h)` où `h` domine `n` définit une boucle naturelle avec `h` comme en-tête. Retourne `Vec<NaturalLoop>`.

`find_convergence(block, cond_block)` trouve le post-dominateur d'un branchement, utilisé durant la structuration pour déterminer où un if/else se rejoint.

---

## Structuration du GFC

`structure_function(func)` convertit le GFC plat en arbre structuré `SNode` adapté à la génération de code sans `goto`.

**Algorithme :**

1. Parcours en profondeur pour identifier les arcs retour (en-têtes de boucles).
2. Filtrage des arcs retour pour obtenir un DAG.
3. Ordre topologique sur le DAG.
4. Pour chaque nœud dans l'ordre :
   - S'il s'agit d'un en-tête de boucle → émettre `Loop { cond, body }`.
   - S'il se termine par un branchement conditionnel → trouver le point de convergence, émettre `IfElse`.
   - Sinon → émettre `Block`.
5. Les séquences de nœuds sans branchement → réduites en `Seq`.

Le `mnemonic` stocké dans les terminateurs `Branch` est utilisé pour construire une `CondExpr` qui préserve la sémantique de la condition d'origine (p. ex. `jl` → inférieur signé).

---

## Récupération de chaînes

`StringRecovery` effectue une extraction de chaînes en plusieurs étapes à partir d'un binaire.

```rust
pub struct StringRecovery<'a> {
    obj: &'a BinaryObject,
    string_table: &'a StringTable,
}
```

**Étapes :**

1. **Balayage `.rodata`** — `apply_rodata_strings()` trouve les séquences ASCII terminées par un octet nul de longueur ≥ 4 dans les sections de données en lecture seule.
2. **Balayage exhaustif** — `recover_strings_from_binary()` parcourt toutes les sections de données ; essaie ASCII, UTF-8, UTF-16 LE/BE, UTF-32 LE/BE.
3. **Récupération tenant compte du GFC** — `recover_strings_with_cfg()` croise les chaînes découvertes avec les instructions qui chargent leurs adresses, améliorant les scores de confiance.

Chaque chaîne récupérée contient :

```rust
pub struct RecoveredString {
    pub content: String,
    pub address: u64,
    pub size: usize,
    pub encoding: StringEncoding,
    pub references: Vec<u64>,   // adresses des instructions qui chargent cette chaîne
    pub confidence: f32,        // 0,0 – 1,0
}
```

Variantes de `StringEncoding` : `Ascii`, `Utf8`, `Utf16Le`, `Utf16Be`, `Utf32Le`, `Utf32Be`, `Binary`.

---

## Graphe d'appel

```rust
pub fn build_call_graph(module: &IrModule) -> CallGraph
```

Parcourt chaque `Stmt::Assign` dont le `rhs` est un `Expr::Call`. Pour chaque appel :
- Appels directs (`CallTarget::Direct(addr)`) → ajout d'un arc vers la fonction à cette adresse.
- Appels nommés (`CallTarget::Named(name)`) → ajout d'un arc vers un nœud externe pour `name`.
- Appels indirects → enregistrés mais non connectés (cible inconnue statiquement).

`CallGraph` est un `petgraph::DiGraph<CgFunction, CgEdge>` où `CgEdge::sites` compte le nombre de sites d'appel reliant la même paire appelant/appelé.

L'interface graphique utilise ce graphe pour la visualisation interactive du graphe d'appel dans `rustdec-gui`.
