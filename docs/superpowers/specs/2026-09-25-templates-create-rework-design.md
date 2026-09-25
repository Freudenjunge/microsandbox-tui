# Templates + Create-Dialog-Rework — Design

Datum: 2026-09-25 · Status: Entwurf zur Review (Brainstorming-Ergebnis, alle Abschnitte im Chat bestätigt)

## Zweck

Der Create-Flow bekommt zwei Ziele:

1. **Schnellstart über Templates**: häufig genutzte Setups (Built-ins: `Opencode`,
   `Opencode2`, `Shell`; eigene hinzu) starten in wenigen Tasten — Template wählen,
   `Enter`, Sandbox entsteht.
2. **Create-Dialog optisch neu ordnen** (Docker-sbx als Referenz): Template-Auswahl
   als Master-Detail-Einstieg (gewählte Variante C), Bearbeiten im eigenen
   Vollbild-Formular mit gruppierten Abschnitten (gewählte Variante 3).

Erfolgskriterium: eine neue Sandbox aus dem passendsten Built-in in ≤ 3 Tastendrücken;
neue Templates lassen sich ohne Codekenntnis erzeugen (im TUI speichern oder eine
TOML-Datei schreiben).

## Non-Goals (v1)

- `rootfs = "snapshot:…"` als Rootfs-Quelle (Golden Images): **Feld ist im Format
  reserviert, wird aber nicht unterstützt** — unser `CreateSpec`/Formular kennt
  Root-Disk/Rootfs-Quellen noch nicht.
- Root-Disk-Größe als Feld (CreateSpec unterstützt es nicht).
- Template-Import aus msb-Konfigurationen, Teilen/Sync, Editor im TUI.
- Exec-Shell „nativer anfühlen“ — eigenes Folgethema (separater Brainstorm/Spec).
- Snapshots-View / Volumes-View — bleiben eigenständige Roadmap-Punkte.

## 1. Template-Datenmodell & Dateiformat

Ein Template = Metadaten + die Felder eines `CreateSpec`. Eine Datei pro Template,
TOML:

```toml
# ~/.config/microsandbox-tui/templates/opencode.toml
[meta]
name = "Opencode"                # Anzeigename; Dateiname (ohne .toml) = id
description = "Node-Image + CWD-Mount, um opencode-ai in der Sandbox zu nutzen"

[spec]
# Spec-Felder — 1:1 die Formular-Felder, alle optional außer image
image = "node:22-alpine"         # Pflicht
name = "opencode"                # Namens-Muster; Kollision → Auto-Suffix -2, -3 …
cpus = 4                         # optional
memory = "2G"                    # optional, Parser wie Formular (parse_memory_mib)
workdir = "/workspace"           # optional
mount_cwd = true                 # optional, Default true
ports = ["127.0.0.1:3000:3000"]  # optional, Format wie Formular-Eingabe
volumes = []                     # optional, `SOURCE:DEST[:OPTIONS]`
env = ["TERM=xterm-256color"]    # optional, `KEY=VALUE`
labels = {}                      # optional, `KEY=VALUE`
net_profile = "public"           # optional, Default `public`
net_rules = []                   # optional, `allow@host`-Tokens
```

- Serde-Struktur: `Template { meta: TemplateMeta, spec: TemplateSpec }`.
  `TemplateSpec` trägt Optionen mit denselben Defaults wie das Formular;
  `image` ist Pflicht (Fehler beim Laden, wenn fehlt/leer).
- Validierung beim Laden mit den existierenden Pure-Helpers
  (`parse_memory_mib`, Port-Parser aus `models.rs`): ungültige Werte →
  Template wird **übersprungen**, Warnung an die Statuszeile, kein Crash.
- Reserviert, in v1 nicht geparst: `rootfs = "snapshot:<group>/<name>"`.
  Der Parser akzeptiert/ignoriert es (mit Warnung) so, dass spätere
  Unterstützung kein Formatwechsel ist.

## 2. Persistenz & Built-ins

- Verzeichnis: `~/.config/microsandbox-tui/templates/*.toml`
  (Auflösung: `XDG_CONFIG_HOME`, sonst `~/.config`; Helper in `template.rs`).
- Built-ins leben im Repo unter `src/templates/*.toml`
  (`opencode.toml`, `opencode2.toml`, `shell.toml`) und werden per
  `include_str!` in `template.rs` eingebettet. Identisches Format — sie sind
  zugleich Dokumentation und Kopiervorlage.
- Laden: Built-ins + Nutzer-Dateien zusammenführen; **bei gleicher id
  (Dateiname) gewinnt die Nutzer-Datei** (Built-ins sind überschreibbar,
  nicht löschbar). Lesen beim TUI-Start (auf einem `tokio`-Task, Ergebnis
  als `AppEvent::TemplatesLoaded(Vec<Template>)` — Muster wie
  `ImagesUpdated`); nach jedem Speichern/Löschen neu einlesen.
- Schreiben: `Ctrl+S` im Formular → Dialog (Name, Beschreibung) →
  `<slug>.toml` (Slug = kebab-case aus Namen; Kollision → Confirm-Dialog
  „Überschreiben?“). Schreiben auf `tokio`-Task, `AppEvent::TemplateSaved`.

## 3. UI — Einstieg (Variante C: Master-Detail)

```
┌─ Templates ──────┬─ Vorschau: Opencode ────────────────┐
│                  │ Image    node:22-alpine             │
│  ▶ Opencode      │ Name     opencode (auto: -2, -3 …)  │
│    Opencode2     │ CPU      4          Mem  2G         │
│    Shell         │ Mount    CWD → /workspace           │
│                  │ Ports    127.0.0.1:3000→3000        │
│                  │ Env      TERM=xterm-256color        │
│  e editieren     │ Net      public                     │
├──────────────────┴─────────────────────────────────────┤
│ Enter: erstellen  e: anpassen  n: neu  d: löschen  Esc │
└────────────────────────────────────────────────────────┘
```

- Links: alle Templates (Nutzer zuerst, dann Built-ins), Cursor-Auswahl,
  Anzeige Name + Beschreibung.
- Rechts: **effektive Werte** der Vorschau — was tatsächlich erstellt würde:
  Template-Wert oder Formular-/Runtime-Default, nicht gesetzte Felder
  ausgegraut mit „(default)“-Markierung. Pure Funktion
  `preview_values(template, cwd) -> Vec<(label, String, is_default)>`,
  unit-getestet. Image-Größe, wenn gecacht, hinter der Referenz.
- Keys: `↑/↓` wählen · `Enter` erstellen · `e` Formular vorgefüllt ·
  `n` leeres Formular · `d` Nutzer-Template löschen (Confirm; Built-in →
  Hinweis „built-in — Datei im Template-Verzeichnis anlegen, um zu
  überschreiben“) · `Esc` zurück ins Dashboard.
- Auto-Name: `resolve_name(template, existing_names) -> Option<String>`
  — Name-Muster belegt → erstes freies `name-2`, `name-3`, …; kein
  `name` im Template → `None` (SDK generiert).
- Kleine Terminals: Breite < 72 Spalten → gestapelt (Liste oben, Vorschau
  unten); Höhe < 20 Zeilen → nur Liste. Pure Layout-Entscheidung
  (`template_layout(width, height) -> LayoutKind`), getestet.

## 4. UI — Vollbild-Formular (gruppiert)

Ersetzt Quick/Advanced (`Ctrl+A` entfällt). Abschnitte mit Headern:

- **Basis** — Image (mit Picker: Scrolling, Size, Dedupe wie gebaut), Name
- **Ressourcen** — CPU, Memory
- **Mounts** — `[x]` CWD mounten, Workdir, Volumes
- **Netzwerk** — Profil (public/private/host), Ports, Rules
- **Sonstiges** — Env, Labels

- Navigation wie heute (`Tab`/`↑↓` Feldwechsel), Listenfelder mit den
  bestehenden `+`/`-`-Interaktionen; `Enter` auf dem letzten Feld = erstellen.
- `Ctrl+S` — „Als Template speichern“: Dialog Name + Beschreibung →
  schreibt die TOML-Datei (siehe §2).
- `Esc` — zurück zur Template-Auswahl (kein Zustands-Merging; die
  C-Ansicht befüllt das Formular bei Bedarf frisch).
- Kleine Terminals: dichte Paarzeilen (CPU+Mem) brechen auf einspaltig;
  Abschnitts-Header bleiben.
- Erstellen läuft detached auf `tokio`-Task wie heute (Status/Spinner),
  Rückkehr aufs Dashboard.

## 5. Architektur & Module

- `src/template.rs` — neu, **pure + I/O-Schale**:
  - Datentypen `Template`, `TemplateMeta`, `TemplateSpec` (serde TOML)
  - `load_all(dir) -> Vec<Template>` (Merge-Regel), `load_builtins()`
  - `write_template(dir, &Template) -> Result<PathBuf>`
  - `to_create_spec(&Template) -> CreateSpec` (Mapping, alle Felder)
  - `resolve_name(...)`, `preview_values(...)`, Layout-Entscheidungen
  - Unit-getestet mit TOML-Fixtures (`src/fixtures/templates/*.toml`)
- `src/models.rs` — keine Änderung (Template ist kein SDK-Typ);
  `CreateSpec` bleibt in `src/backend/mod.rs`.
- `src/app.rs` — `View::Create` erhält zwei Zustände
  (`TemplateSelect` ↔ `Form(CreateForm)`); `App.templates: Vec<Template>`;
  neue Events `TemplatesLoaded(Vec<Template>)`, `TemplateSaved(String)`,
  `TemplateDeleted(String)`. Create-Pfad unverändert
  (`CreateSpec → backend.create_detached`, detached-Regel gilt).
- `src/ui/create.rs` — Formular-Umbau (gruppierte Abschnitte,
  `field_count`/Feldliste neu, Ctrl+S-Autoring), bestehende Picker-Logik
  bleibt.
- `src/ui/template_picker.rs` — neu: Rendering C-Ansicht (Liste + Vorschau),
  nutzt die pure Funktionen aus `template.rs`.
- UI-Code bleibt SDK-frei; Template-I/O geht durch `tokio`-Tasks in
  `main.rs`/`actions.rs`-Stil.

## 6. Testing (TDD)

Reihenfolge: je Verhalten erst der fehlgeschlagene Test, dann Implementierung.

1. Parser: gültiges Fixture → alle Felder; fehlendes `image` → Fehler;
   ungültige `memory`/`ports` → übersprungen mit Warnung; unbekannte
   Felder ignorieren; `rootfs`-Feld reserviert (geladen ohne Wirkung +
   Warnung in v1).
2. Merge: Nutzer-Datei shadowt Built-in gleicher id; Reihenfolge
   (Nutzer zuerst, dann Built-ins).
3. `to_create_spec`: jedes Feld gemappt (oder bewusst Default),
   Defaults deckungsgleich mit Formular-Defaults.
4. `resolve_name`: freier Name, Kollision → `-2`, `-3`; ohne Name → `None`.
5. `preview_values`: Template-Wert vs Default-Markierung korrekt.
6. Roundtrip: `write_template` → `load_all` liefert gleiches Template.
7. Formular → Template (Ctrl+S-Pfad): `form_to_template` übernimmt alle
   Felder; Slug-Erzeugung aus dem Namen.
8. Kleine-Terminal-Layout-Entscheidung (pure):
   `template_layout(71, 24)` = gestapelt, `template_layout(72, 24)` =
   Master-Detail, `template_layout(72, 19)` = nur Liste.
9. UI-Rendering selbst: nicht unit-getestet (Projektregel) — Kompilieren +
   manueller Smoke-Test.

## 7. Dokumentation & Roadmap

- `docs/DESIGN.md`: Phase 2 um die Punkte **11. Templates** (Built-ins +
  Nutzerdateien, Master-Detail-Einstieg) und **12. Create-Formular-Umbau**
  (gruppierte Abschnitte, Ctrl+S-Authoring) erweitern; Keybinding-Tabelle
  (`c`, `e`, `n`, `d`, `Ctrl+S`, entfallenes `Ctrl+A`) aktualisieren.
- `docs/PLAN.md`: Aufgabenabschnitt „Templates + Create-Rework“ anlegen;
  jede abgeschlossene Aufgabe dort abhaken (gleicher Commit wie die Arbeit).

## 8. Built-in-Inhalte (Platzhalter, bewusst editierbar)

| id | image | Besonderheiten |
|----|-------|----------------|
| `opencode` | `node:22-alpine` | CWD-Mount, workdir `/workspace`, `TERM=xterm-256color` |
| `opencode2` | `node:24-alpine` | CWD-Mount, `cpus 8`, `memory 4G` (Varianten-Beispiel) |
| `shell` | `alpine` | CWD-Mount, generischer Einstieg |

Die Werte sind Absichtserklärungen, keine Endgültigkeit — die TOML-Dateien
sind dafür da, sie anzupassen.