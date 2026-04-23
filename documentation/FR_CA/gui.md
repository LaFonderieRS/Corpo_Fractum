# Guide d'utilisation de l'interface graphique

`rustdec-gui` est l'application de bureau GTK4. Elle offre le même pipeline de décompilation que l'ILC dans une interface interactive à trois volets.

---

## Compilation et démarrage

```bash
# Dépendances système (en-têtes GTK4 + Cairo)
sudo apt install libgtk-4-dev libcairo2-dev pkg-config   # Debian/Ubuntu
sudo pacman -S gtk4 cairo pkgconf                         # Arch
sudo dnf install gtk4-devel cairo-devel                   # Fedora

# Compilation (défaut — sans volet console)
cargo build --release -p rustdec-gui

# Avec le volet console sous le code et le graphe
cargo build --release -p rustdec-gui --features console-bottom

# Avec le volet console comme onglet à côté du graphe d'appel
cargo build --release -p rustdec-gui --features console-tab

# Démarrage
./target/release/corpo-fractum
```

---

## Disposition de la fenêtre

```
┌──────────────────────────────────────────────────────────────────┐
│  Barre titre   [ Ouvrir… ]                [ C ▾ ]   [ À propos ] │
├─────────────────┬────────────────────────────────────────────────┤
│                 │                                                 │
│   Explorateur   │   Volet de code                                │
│   (240 px)      │                                                 │
│                 │   Pseudo-code décompilé ou vidage hexadécimal  │
│   SECTIONS      │                                                 │
│   [code] .text  │                                                 │
│   [data] .bss   ├─────────────────────────────────────────────────┤
│   …             │                                                 │
│                 │   Graphe d'appel                               │
│   FONCTIONS (N) │                                                 │
│   [script compl]│   Déplacer · Zoomer · Cliquer                  │
│   main          │                                                 │
│   compute       │                                                 │
│   …             │                                                 │
│                 │                                                 │
│   🔍 Chercher…  │                                                 │
└─────────────────┴─────────────────────────────────────────────────┘
```

Taille de fenêtre par défaut : 1400 × 900. Tous les séparateurs de volets sont déplaçables.

---

## Ouverture d'un binaire

Cliquez sur **Ouvrir…** dans la barre de titre et sélectionnez n'importe quel binaire ELF, PE ou Mach-O. L'analyse démarre immédiatement en arrière-plan. Le volet explorateur se remplit de façon incrémentielle au fur et à mesure que les fonctions sont élevées — il n'est pas nécessaire d'attendre la fin de l'analyse complète pour commencer à consulter les résultats.

---

## Volet explorateur

**Sections** — cliquez sur n'importe quel bouton de section pour en afficher le contenu dans le volet de code. Chaque bouton montre :
- Un badge de couleur indiquant le type : `code`, `rodata`, `data`, `bss`, `debug` ou `other`.
- Le nom de la section.
- La taille virtuelle en octets.

**Fonctions** — la liste s'allonge au fur et à mesure que chaque fonction est décompilée. Le compteur `FONCTIONS (N)` se met à jour en temps réel.

**Recherche** — saisissez dans la barre de recherche pour filtrer les noms de fonctions (correspondance partielle sans distinction de casse).

**Script complet** — le bouton dans l'en-tête des fonctions devient actif une fois l'analyse terminée. Un clic charge toutes les fonctions décompilées, concaténées dans l'ordre d'analyse, dans le volet de code.

---

## Volet de code

Affiche la sortie pour la fonction ou la section actuellement sélectionnée.

**Vue fonction** — pseudo-code avec coloration syntaxique dans le langage choisi dans le menu déroulant de la barre de titre (C, C++ ou Rust). La coloration est appliquée de façon incrémentielle (250 lignes par impulsion d'inactivité) pour maintenir la réactivité de l'interface sur les grosses fonctions.

| Élément | Couleur |
|---|---|
| Mots-clés | bleu, gras |
| Commentaires `//` | vert |
| Adresses hexadécimales | cyan |
| Littéraux de chaînes ASCII | orange |

**Vue section** — pour les sections de données : un vidage hexadécimal (4 premiers Ko) plus les chaînes imprimables extraites avec leurs décalages.

---

## Volet du graphe d'appel

Un graphe interactif rendu par Cairo représentant tous les appels de fonctions dans le binaire.

### Navigation

| Geste | Effet |
|---|---|
| Clic gauche sur un nœud | Sélectionne la fonction ; le volet de code se met à jour immédiatement |
| Glisser (bouton gauche) | Déplace le canevas |
| Molette de défilement | Zoom avant/arrière (plage : 0,1× – 8,0×) ; pivot sous le pointeur |
| Survol | Met en évidence les arcs sortants (vert) et entrants (ambre) du nœud |

### Apparence des nœuds

| Type de nœud | Couleur | Hauteur |
|---|---|---|
| Fonction interne | bleu foncé | 34 – 64 px, proportionnelle au nombre d'instructions |
| Externe / import | sarcelle foncé | 34 px (fixe) |
| Sélectionné | bordure jaune/or | — |

L'opacité des arcs est proportionnelle au nombre de sites d'appel entre les deux fonctions. Les arcs retour (appels récursifs ou cycliques) contournent le côté gauche du graphe.

### Disposition

Le graphe est disposé une seule fois à la réception de l'événement `CallGraphReady` :

1. Tri topologique (Kahn, gère les composantes fortement connexes).
2. Assignation de couches par chemin le plus long.
3. Heuristique barycentrique (une passe avant) pour réduire les croisements d'arcs.
4. Courbes de Bézier cubiques avec des têtes de flèches remplies.

---

## Volet console

Disponible uniquement lorsque compilé avec `console-bottom` ou `console-tab`.

Affiche deux flux de texte :

**Événements du cycle de vie** (du pont) :
- `[info] analyse démarrée : /chemin/vers/binaire`
- `[info] analyse terminée — N fonction(s) élevée(s)`
- `[error] …` en cas d'échec

**Entrées du journal de traçage** de toutes les crates, colorées par niveau :

| Niveau | Couleur |
|---|---|
| INFO | gris |
| DEBUG | bleu-gris |
| WARN | or |
| ERROR | saumon |

La console défile automatiquement jusqu'à la dernière entrée.

---

## Sélection du langage

Le menu déroulant dans la barre de titre change le langage de sortie pour tous les événements `FunctionSelected` et « script complet » suivants. Il ne **relance pas** l'analyse ; le changement de langage s'effectue à l'étape de génération de code et est instantané.

---

## Journalisation vers la sortie standard

Indépendamment du volet console, tous les événements de traçage sont également écrits sur la sortie standard. Filtrez avec `RUSTDEC_LOG` :

```bash
RUSTDEC_LOG=debug ./target/release/corpo-fractum
RUSTDEC_LOG=rustdec_analysis=debug,info ./target/release/corpo-fractum
```

---

## Boîte de dialogue « À propos »

Cliquez sur **À propos** dans la barre de titre pour afficher la version, la description, la liste des contributeurs et les informations de compilation (version GTK, chaîne d'outils Rust).
