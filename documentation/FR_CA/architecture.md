# Architecture

## Pipeline

Chaque exécution d'analyse suit ce pipeline linéaire. Chaque étape est une crate indépendante avec un contrat d'entrée/sortie bien défini.

```
┌──────────────────────────────────────────────────────────────────┐
│  Fichier binaire (.elf / .exe / .dylib / …)                      │
└───────────────────────────┬──────────────────────────────────────┘
                            │
              ┌─────────────▼──────────────┐
              │       rustdec-loader        │
              │  ELF · PE · Mach-O · DWARF  │
              │  → BinaryObject             │
              │  → StringTable              │
              │  → DwarfInfo                │
              └─────────────┬──────────────┘
                            │
              ┌─────────────▼──────────────┐
              │       rustdec-disasm        │
              │  Capstone (multi-arch)      │
              │  → Vec<Instruction>         │
              └─────────────┬──────────────┘
                            │
              ┌─────────────▼──────────────┐
              │      rustdec-analysis       │
              │  détection de fonctions     │
              │  construction du GFC (3 passes) │
              │  dominance + boucles        │
              │  structuration du GFC       │
              │  récupération de chaînes    │
              │  graphe d'appel             │
              │  → IrModule (GFC + stmts)   │
              └─────────────┬──────────────┘
                            │  (par fonction, en parallèle via rayon)
              ┌─────────────▼──────────────┐
              │        rustdec-lift         │
              │  x86-64 → RI SSA            │
              │  analyse de cadre de pile   │
              │  élimination du code mort   │
              │  annotation de symboles     │
              │  → IrFunction (SSA)         │
              └─────────────┬──────────────┘
                            │
              ┌─────────────▼──────────────┐
              │       rustdec-codegen       │
              │  émission C / C++ / Rust    │
              │  recherche de signatures libc │
              │  → Vec<(nom, source)>       │
              └─────────────┬──────────────┘
                            │
                  ┌─────────┴─────────┐
                  │                   │
        ┌─────────▼──────┐   ┌────────▼────────┐
        │  rustdec-cli   │   │  rustdec-gui     │
        │  ILC sans IHM  │   │  bureau GTK4     │
        └────────────────┘   └─────────────────┘
```

---

## Responsabilités des crates

| Crate | Responsabilité | Dépendances clés |
|---|---|---|
| `rustdec-loader` | Analyser ELF/PE/Mach-O en `BinaryObject` ; informations de débogage DWARF | goblin, gimli |
| `rustdec-disasm` | Désassembler les octets bruts en `Vec<Instruction>` | capstone-rs |
| `rustdec-ir` | Définir tous les types de la RI : `IrType`, `Stmt`, `Expr`, `IrFunction`, `IrModule` | petgraph |
| `rustdec-lift` | Élever les instructions x86-64 vers la SSA ; analyser les cadres de pile | rustdec-ir, rustdec-disasm |
| `rustdec-analysis` | Orchestrer le pipeline complet ; GFC, structuration, récupération de chaînes | rayon, petgraph |
| `rustdec-codegen` | Émettre du pseudo-code à partir d'un `IrModule` | rustdec-ir, rustdec-lift |
| `rustdec-cli` | Analyser les arguments ILC ; piloter le pipeline sans interface graphique | clap, anyhow |
| `rustdec-gui` | Application de bureau GTK4 ; pont asynchrone vers le moteur Tokio | gtk4, cairo-rs, tokio |
| `rustdec-bench` | Banc d'essai pour le pipeline d'analyse | clap, serde |
| `rustdec-lua` | Moteur de greffons Lua (ébauche) | mlua |

---

## Parallélisme

Deux niveaux de parallélisme sont utilisés :

**Niveau 1 — parallélisme des données (rayon::join) :**  
Le désassemblage et l'extraction des chaînes s'exécutent simultanément sur le même objet binaire.

**Niveau 2 — parallélisme par fonction (rayon::par_iter) :**  
La construction du GFC et l'élévation s'exécutent en parallèle sur toutes les fonctions détectées. Chaque fonction est indépendante ; les résultats sont collectés et assemblés dans un `IrModule`.

L'interface graphique ajoute un troisième niveau : l'ensemble du pipeline d'analyse s'exécute dans un appel `tokio::task::spawn_blocking` afin que le fil principal GTK ne soit jamais bloqué.

---

## Gestion des erreurs

Chaque crate définit sa propre énumération d'erreurs via `thiserror`. Les erreurs se propagent sous forme de `Result<T, E>` — aucune panique n'est possible dans le chemin critique. L'ILC et l'interface graphique interceptent les erreurs au niveau supérieur et les affichent à l'utilisateur.

---

## Journalisation

Toutes les crates utilisent les macros `tracing` (`trace!`, `debug!`, `info!`, `warn!`, `error!`). L'abonné est configuré par le point d'entrée (ILC ou interface graphique). Le filtre est contrôlé par la variable d'environnement `RUSTDEC_LOG` (même syntaxe que `RUST_LOG`).

---

## Principes de conception

**RI indépendante de l'architecture.**  
Après élévation, aucun registre ni concept propre à une architecture ne subsiste dans la RI. Toutes les passes ultérieures — structuration, génération de code — sont de pures opérations sur la RI.

**Données immuables partagées via `Arc`.**  
`IrTypeRef = Arc<IrType>` permet au même nœud de type d'être référencé par des milliers de variables SSA sans copie. `Arc<str>` interne les noms de symboles et d'imports dans tout le module.

**Pas d'`unsafe` dans le chemin critique.**  
Les interfaces étrangères vers Capstone et GTK4 sont encapsulées dans leurs crates d'enrobage respectives. Les crates d'analyse et de RI ne contiennent aucun `unsafe`.

**Sortie incrémentielle.**  
L'interface graphique reçoit des événements `AnalysisFunctionReady` au fur et à mesure que chaque fonction est traitée, permettant à l'utilisateur de consulter les résultats avant la fin de l'analyse complète.
