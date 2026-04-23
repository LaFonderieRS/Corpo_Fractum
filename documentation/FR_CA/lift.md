# Élévateur

`rustdec-lift` traduit les instructions machine x86-64 en instructions de RI SSA. Il opère sur une fonction à la fois et est invoqué par `rustdec-analysis` en parallèle sur toutes les fonctions détectées.

---

## Point d'entrée

```rust
pub fn lift_function(func: &mut IrFunction, insns: &[Instruction], symbols: &SymbolMap)
```

L'appelant fournit :
- `func` — une `IrFunction` dont le GFC a déjà été construit (graphe de blocs de base, sans instructions encore).
- `insns` — la tranche d'instructions aplatie pour toute la fonction.
- `symbols` — la table de symboles utilisée pour la résolution constante → symbole.

Après cet appel, chaque bloc de base du GFC a ses `stmts` et son `terminator` remplis.

---

## Pipeline interne de `lift_function`

```
1. Élévation par bloc (en parallèle via rayon)
     lift_block_with_regs(insns_du_bloc) → (stmts, ret_var, reg_names)

2. Analyse de cadre de pile
     analyse_frame(func)
     → découverte des emplacements de pile, nommage des locales/args/registres sauvegardés
     → réécriture des accès [rbp±N] / [rsp±N] vers des emplacements nommés
     → détection des tableaux contigus, réécriture en ArrayAccess

3. Résolution constante → symbole
     parcours de toutes les valeurs Const dans la table de symboles
     → remplacement des adresses correspondantes par Expr::Symbol{kind, name}

4. Élimination du code mort
     marquage des instructions Assign inutilisées en Nop

5. Inférence du type de retour
     inspection des terminateurs Return pour déterminer ret_ty

6. Inférence de l'arité ABI
     comptage des registres d'arguments distincts utilisés → définition de func.params

7. Élimination du canari de pile
     suppression des chemins __stack_chk_fail et de leurs gardes
```

---

## Table des registres

`RegisterTable` associe les registres physiques x86-64 à des identifiants de variables SSA. Elle gère la hiérarchie complète des alias de registres :

```
rax (64) → eax (32) → ax (16) → al / ah (8)
rcx, rdx, rbx, rsp, rbp, rsi, rdi
r8 – r15  (avec les alias r8d, r8w, r8b)
xmm0 – xmm15
```

La lecture d'un alias étroit (p. ex. `eax`) effectue une extension par zéro depuis la variable 64 bits courante. L'écriture dans un registre 32 bits effectue une extension par zéro vers 64 bits (règle ABI x86-64). L'écriture dans un alias 8/16 bits insère un masquage et un OU.

L'amorçage ABI pré-alloue les registres System V x86-64 avant le début de l'élévation :

| Classe | Registres |
|---|---|
| Arguments | rdi, rsi, rdx, rcx, r8, r9 |
| Valeur de retour | rax |
| Callee-saved | rbx, r12, r13, r14, r15 |
| Pointeurs de cadre | rsp, rbp |

---

## Traqueur d'indicateurs

`FlagTracker` maintient des variables SSA actives pour les quatre indicateurs arithmétiques après chaque instruction qui les modifie :

| Indicateur | Mis à jour par |
|---|---|
| ZF (zéro) | add, sub, cmp, test, and, or, xor, inc, dec, … |
| SF (signe) | identique |
| CF (retenue) | add, sub, adc, sbb, shl, shr, … |
| OF (débordement) | add, sub, imul, … |

Les instructions de saut conditionnel (`jz`, `jnz`, `jl`, `jge`, `jb`, …) lisent la variable d'indicateur appropriée et émettent un terminateur `Branch` avec cette valeur comme condition. Le mnémonique d'origine est préservé dans `Branch::mnemonic`.

---

## Couverture des instructions

`lift_block_with_regs` prend en charge plus de 100 mnémoniques x86-64, notamment :

**Mouvement de données :** `mov`, `movzx`, `movsx`, `movsxd`, `lea`, `push`, `pop`, `xchg`

**Arithmétique :** `add`, `sub`, `imul`, `mul`, `idiv`, `div`, `inc`, `dec`, `neg`

**Logique / décalages :** `and`, `or`, `xor`, `not`, `shl`, `shr`, `sar`, `rol`, `ror`

**Comparaisons :** `cmp`, `test`, famille `setcc`

**Flot de contrôle :** `call`, `ret`, `jmp`, famille `jcc` (16 conditions), `ud2`, `hlt`

**Chaînes / SSE :** `rep movs`, `rep stos`, `movss`, `movsd`, `addss`, `addsd`, chargements vectoriels de base

Les instructions non reconnues produisent `Expr::Opaque(mnémonique)` afin que l'élévation n'échoue jamais.

---

## Analyse de cadre de pile

`analyse_frame` est une passe distincte qui s'exécute après l'élévation par bloc. Elle :

### Détection du prologue

Identifie le prologue de cadre x86-64 standard :
```asm
push rbp
mov  rbp, rsp
sub  rsp, N        ← extrait frame_size = N
```

Supprime les affectations du prologue de la RI (il s'agit de code de protocole ABI, pas de logique).

### Détection de l'épilogue

Supprime les patterns `leave` (équivalent à `mov rsp, rbp; pop rbp`) et `pop rbp` seul.

### Élimination des registres callee-saved

Supprime les paires push/pop pour rbx, r12, r13, r14, r15 à l'entrée et à la sortie de la fonction.

### Découverte des emplacements de pile

Parcourt chaque expression `Load` et `Store` à la recherche des patterns `[rbp ± décalage]` et `[rsp ± décalage]`. Chaque décalage unique devient un `StackSlot` nommé :

| Plage de décalage | Origine | Pattern de nom |
|---|---|---|
| rbp - N (N > 0) | `Local` | `local_0`, `local_1`, … |
| rbp + N (N > 8) | `StackArg` | `arg_0`, `arg_1`, … |
| Déversements callee-saved | `SavedReg` | `saved_rbx`, `saved_r12`, … |

### Détection des tableaux

Lorsque plusieurs emplacements de pile adjacents ont un type identique et un pas uniforme, ils sont fusionnés en un seul emplacement avec `ArrayInfo { count, stride }`. Les accès sont réécrits en expressions `ArrayAccess { name, index, elem_ty }`.

Exemple :
```c
// avant détection
local_0 = …;  local_8 = …;  local_16 = …;

// après détection
buf[0] = …;   buf[1] = …;   buf[2] = …;
```

### Zone rouge

Les fonctions feuilles sur x86-64 peuvent utiliser la zone rouge de 128 octets sous RSP sans ajuster le pointeur de pile. L'analyseur de cadre détecte ce pattern et nomme les emplacements en conséquence.

### Allocation dynamique

`sub rsp, reg` (où le membre droit est une variable, non une constante) est reconnu comme un `alloca` dynamique et marqué séparément.

---

## Propagation de copies

Après l'analyse de cadre, l'élévateur effectue une passe légère de propagation de copies sur les emplacements de pile : si un emplacement est écrit exactement une fois avec une constante et lu sans écriture intermédiaire, la référence à l'emplacement est remplacée par la constante en ligne.

---

## Canari de pile

Si `__stack_chk_fail` est détecté comme cible d'appel, l'élévateur supprime toute la séquence de vérification du canari :
- Le chargement `fs:0x28` dans un emplacement local.
- La comparaison XOR à la sortie de la fonction.
- Le branchement conditionnel vers `__stack_chk_fail`.
- Le bloc d'appel `__stack_chk_fail` lui-même.

Cela nettoie considérablement la sortie pour tout binaire compilé avec `-fstack-protector`.
