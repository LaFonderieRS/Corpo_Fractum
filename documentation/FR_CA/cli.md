# Référence de l'interface en ligne de commande

`rustdec-cli` fournit une interface sans interface graphique pour le pipeline d'analyse complet. Le binaire se nomme `corpo-fractum-cli`.

---

## Compilation

```bash
cargo build --release -p rustdec-cli
# → target/release/corpo-fractum-cli
```

---

## Synopsis

```
corpo-fractum-cli [OPTIONS] <BINAIRE>
```

---

## Options

| Drapeau | Type | Défaut | Description |
|---|---|---|---|
| `-l, --lang <LANG>` | `c` \| `cpp` \| `rust` | `c` | Langage de sortie |
| `-o, --output <RÉP>` | chemin | — | Écrire un fichier `.c`/`.cpp`/`.rs` par fonction dans RÉP |
| `-F, --function <NOM>` | chaîne (répétable) | — | Décompiler uniquement la ou les fonctions nommées |
| `--list` | drapeau | — | Lister les fonctions détectées et quitter sans élévation |
| `--emit-ir` | drapeau | — | Afficher la RI SSA élevée plutôt que le code décompilé |
| `-v` / `-vv` / `-vvv` | drapeau (cumulable) | — | Verbosité : info / debug / trace |

---

## Sous-commandes (implicites)

Le mode est sélectionné selon les drapeaux présents :

| Drapeaux présents | Mode | Ce qui s'exécute |
|---|---|---|
| `--list` | liste | chargeur + désassembleur + détection de fonctions seulement |
| `--emit-ir` | affichage RI | pipeline complet jusqu'à l'élévation incluse |
| *(aucun)* | décompilation | pipeline complet incluant la génération de code |

---

## Exemples

```bash
# Lister toutes les fonctions détectées avec leurs adresses d'entrée
corpo-fractum-cli --list ./binaire_cible

# Décompiler une seule fonction en C (affiché sur la sortie standard)
corpo-fractum-cli -F main ./binaire_cible

# Décompiler deux fonctions en Rust (affiché sur la sortie standard)
corpo-fractum-cli -l rust -F main -F compute ./binaire_cible

# Décompiler tout et écrire un fichier par fonction
corpo-fractum-cli -o ./out ./binaire_cible

# Afficher la RI SSA brute pour déboguer une fonction précise
corpo-fractum-cli --emit-ir -F parse_args ./binaire_cible

# Journal d'analyse détaillé (niveau debug)
corpo-fractum-cli -vv ./binaire_cible

# Remplacer le filtre de journal par la variable d'environnement
RUSTDEC_LOG=rustdec_lift=trace corpo-fractum-cli ./binaire_cible
```

---

## Format de sortie

### Mode `--list`

```
0x00401180  main
0x00401240  compute
0x004012f0  parse_args
0x00401390  sub_401390
```

### Mode par défaut (décompilation), sortie standard

```c
// ── main ──
uint64_t main(int argc, char **argv) {
    uint64_t local_0;
    …
}

// ── compute ──
uint64_t compute(uint64_t arg_0) {
    …
}
```

### Mode `--output <RÉP>`

Un fichier par fonction, nommé d'après la fonction :

```
out/
├── main.c
├── compute.c
└── parse_args.c
```

Les fonctions présentes dans la liste de filtrage CRT (p. ex. `_start`, `__libc_csu_init`) sont toujours exclues.

---

## Journalisation

La sortie du journal est dirigée vers la sortie d'erreur et est indépendante du code décompilé sur la sortie standard. Cela permet de rediriger la sortie standard sans pollution :

```bash
corpo-fractum-cli ./binaire 2>/dev/null | grep "uint64_t"
```

La variable d'environnement `RUSTDEC_LOG` accepte la même syntaxe de filtrage que `RUST_LOG` (de la crate `tracing-subscriber`) :

```bash
RUSTDEC_LOG=debug                         # toutes les crates au niveau debug
RUSTDEC_LOG=rustdec_lift=trace,info       # lift en trace, reste en info
RUSTDEC_LOG=rustdec_analysis=debug        # analyse seulement
```

---

## Codes de retour

| Code | Signification |
|---|---|
| `0` | Succès |
| `1` | Erreur d'analyse (binaire invalide, format non supporté, etc.) |
| `2` | Erreur d'entrée/sortie (fichier introuvable, répertoire de sortie non accessible en écriture) |
