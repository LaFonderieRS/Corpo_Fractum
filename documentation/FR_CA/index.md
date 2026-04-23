# Corpo Fractum — Documentation

Décompilateur binaire écrit en Rust : ELF/PE/Mach-O → RI SSA → pseudo-code C / C++ / Rust.

---

## Table des matières

| Document | Description |
|---|---|
| [Architecture](architecture.md) | Vue d'ensemble du pipeline, graphe des crates, principes de conception |
| [Représentation intermédiaire](ir.md) | RI SSA : types, valeurs, instructions, flot de contrôle |
| [Élévateur](lift.md) | x86-64 → RI SSA : table des registres, indicateurs, analyse de cadre, détection de tableaux |
| [Analyse](analysis.md) | Construction du GFC, dominance, structuration, récupération de chaînes, graphe d'appel |
| [Génération de code](codegen.md) | Moteurs C / C++ / Rust, signatures libc, filtrage CRT |
| [Interface en ligne de commande](cli.md) | Référence de l'ILC |
| [Interface graphique](gui.md) | Guide d'utilisation de l'interface GTK4 |

---

## Orientation rapide

```
Fichier binaire  ──►  rustdec-loader   analyse le format + DWARF
                 ──►  rustdec-disasm   désassemble en instructions
                 ──►  rustdec-analysis construit le GFC, détecte les fonctions, structure le flot
                 ──►  rustdec-lift     élève x86-64 → RI SSA, nomme les emplacements de pile
                 ──►  rustdec-codegen  émet le source C / C++ / Rust
                 ──►  rustdec-cli      ILC sans interface graphique
                 ──►  rustdec-gui      application de bureau GTK4
```

Toutes les crates se trouvent directement à la racine de l'espace de travail — aucun sous-répertoire intermédiaire.
