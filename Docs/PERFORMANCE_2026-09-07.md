# Kova Screen: Darstellungsfehler, Ressourcen und Belastungstest

Geprüft am 7. September 2026 unter Windows 11 Pro, Build 26200, auf einem
Intel Core i7-13700K mit 24 logischen Prozessoren. Rust 1.98.1, x64 Release,
Visual Studio 2022. Die Tests arbeiten lokal; der Ressourcen-Probe ruft weder
Upload noch Zwischenablage auf und entfernt seine eigenen Testvideos.

## Änderungen

- Bereichsauswahl wird vollständig in einem wiederverwendeten Hintergrundpuffer
  gezeichnet. Erst das fertige Bild wird angezeigt; der helle Zwischenschritt
  wird nicht mehr sichtbar. Der Puffer wird mit dem Overlay freigegeben.
- Nur noch der Programmcode erstellt das Tray-Icon; die doppelte Erstellung
  aus der Tauri-Konfiguration entfällt.
- Das Schließen des letzten Einstellungs-/Verlaufsfensters lässt die Tray-App
  weiterlaufen. Explizites Beenden bleibt möglich.
- Pause/Stop erhalten eine sichtbare Hover-Fläche. Ein schmaler, durchklickbarer
  Rahmen markiert den tatsächlich aufgenommenen Bereich für MP4/GIF, einschließlich
  Monitor-Clipping und der geraden MP4-Abmessungen. Der Rahmen wird von Aufnahmen
  ausgeschlossen und beim Stoppen entfernt.
- Native Statuskarten unten rechts melden gespeicherte Aufnahmen und Uploadfehler
  auch ohne installierte Windows-Benachrichtigungsregistrierung. Sie respektieren
  die bestehende Benachrichtigungseinstellung, schließen automatisch und benötigen
  keinen WebView. Die Warteschlange ist begrenzt.
  Auf Wunsch anschließend auf 360 × 104 Pixel verkleinert: abgerundete Ecken,
  dezente Kontur, zurückhaltende Textfarben und vier Sekunden Anzeigedauer.
- Der Aufnahme-Thread kopiert den zuletzt gelesenen Frame nicht noch einmal
  vollständig. Er liest nur den neuesten verfügbaren Frame vom Grafikprozessor
  zurück und überspringt diese Arbeit während einer Pause.
- Fehlgeschlagene Overlay-Bitmap-Allokationen geben ihren Grafikkontext frei.
- Media Foundation wird zwischen Aufnahmen wiederverwendet und beim Programmende
  freigegeben, sobald kein Recorder mehr darauf zugreift.

## Native Capture-Lebensdauer

Ein Debugger-Lauf lokalisierte einen wiederholbaren Execute-Access-Violation-Absturz
in einer bereits entladenen `GraphicsCapture.dll`. Windows kann nach `Close()`
noch einen internen Worker abbauen. Die DLL wird deshalb einmal für die
Prozesslebensdauer festgehalten. Ein einzelner COM-MTA-Lebensdauerverweis hält
außerdem die von WinRT zwischengespeicherten Aktivierungsobjekte gültig.
Das sind begrenzte, einmalige Laufzeitressourcen; Capture-Sessions, Frame-Pools,
Bitmaps und D3D-Geräte werden weiterhin nach jeder Aufnahme freigegeben.
Ungültige Monitor-Handles werden vor dem WinRT-Aufruf abgewiesen.

Der dokumentierte Windows-Fall stimmt mit der lokalen Absturzsignatur überein:
[Win32CaptureSample, Issue 99](https://github.com/robmikh/Win32CaptureSample/issues/99).
Die Lebensdauerregeln wurden außerdem gegen
[GraphicsCaptureSession.Close](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.close)
und die [Media-Foundation-Initialisierung](https://learn.microsoft.com/en-us/windows/win32/api/mfreadwrite/nf-mfreadwrite-mfcreatesinkwriterfromurl)
abgeglichen.

## Messungen des nativen Aufnahmepfads

Acht MP4- und acht GIF-Aufnahmen mit jeweils zwei Sekunden und 1280 × 720 Pixeln.
MP4: Standardprofil mit Hardware-Encoding erlaubt; GIF: 15 FPS. Erfasst wurden
Working Set, Private Bytes, Prozess-Handles, GDI-/USER-Objekte und Prozess-CPU-Zeit.
Die CPU-Zahl über alle logischen Prozessoren ist hier die Ein-Kern-Zahl geteilt
durch 24. Diese kurzen Desktop-Aufnahmen sind keine 4K-/Gaming-Leistungszusage.

| Zustand | Working Set | Private Bytes | CPU, Anteil aller logischen Prozessoren |
| --- | ---: | ---: | ---: |
| MP4 während Aufnahme | 87–102 MiB | 124–134 MiB | durchschnittlich ca. 0,92 % |
| GIF während Aufnahme | 71–83 MiB | 72–116 MiB | durchschnittlich ca. 2,49 % |
| Nach Ende der Serie und Ruhephase | 50,71 MiB | 30,50 MiB | keine weitere CPU-Zeit im letzten Ruheintervall |

Die letzten fünf GIF-Stopps lagen konstant bei 534 Handles, vier GDI- und zwei
USER-Objekten. Ein isolierter Capture-Test mit zehn Start-/Stop-Zyklen blieb bei
300 Handles. 20 Recorder-Overlay-Zyklen blieben bei 168 Handles; 100 Durchläufe
der Auswahl-Komposition sowie 20 Statuskarten hinterließen keine zusätzlichen
GDI-/USER-Objekte.

## Offener Befund: Hardware-Encoding

Der isolierte Hardware-Encoder-Probe zeigt weiterhin einen kleinen nativen
Handle-Zuwachs. Die Wiederverwendung der Media-Foundation-Laufzeit reduzierte
den ursprünglichen Zuwachs von ungefähr 48 auf häufig vier Handles je schnell
aufeinanderfolgender Aufnahme, beseitigte ihn aber nicht vollständig. Nach
20 Aufnahmen und 60 Sekunden Ruhe sank der private Speicher auf ca. 16 MiB;
zusätzliche Handles blieben bestehen. Deshalb ist der Hardwarepfad ausdrücklich
**nicht als leakfrei freigegeben**. Welcher native Hardware-Transform bzw.
Treiber die verbleibenden Handles besitzt, ist noch nicht abschließend geklärt.

Mit `hardware_encoding = false` blieb der isolierte Encoder nach dem Aufwärmen
über zehn Aufnahmen konstant bei **397 Handles**. Nach Abschalten der
gemeinsamen Laufzeit: 334 Handles, 25,82 MiB Working Set und 9,89 MiB Private Bytes.
Für den lokalen Benutzertest wurde deshalb Software-Encoding aktiviert.
Alle anderen gespeicherten Einstellungen wurden unverändert übernommen;
die ursprüngliche Datei liegt daneben als
`settings.before-resource-test-20260907-170915.json`.
Der Hardware-Schalter bleibt verfügbar. Dies ist eine lokale Umgehung des
nachgewiesenen Problems, keine Änderung der Encoder-Standardeinstellung für
andere Installationen und keine Behauptung über alle Hardware-Encoder.

## Speicher der tatsächlich gestarteten App

Der finale Release-Build wurde fünfmal über eine zweite Instanz in die
Einstellungen geöffnet und anschließend per `WM_CLOSE` geschlossen. Die
Tray-App blieb dabei aktiv. Nach jedem Schließen verschwanden alle sechs
zugehörigen WebView-Prozesse.

| Zustand | Working Set | Private Bytes | Prozess-Handles |
| --- | ---: | ---: | ---: |
| Frisch gestartet, nur Tray | 14,60 MiB | 2,51 MiB | 215 |
| Einstellungen offen, einschließlich WebView-Prozesse | 357–358 MiB | 162–166 MiB | 387–393 im Hauptprozess |
| Nach fünf Fensterzyklen, nur Tray | 34,39 MiB | 6,45 MiB | 363 |

Ab dem zweiten Schließen blieben Handles bei 363, GDI-Objekte bei 13 und
USER-Objekte bei 15. Die CPU-Messung im anschließenden dreisekündigen
Ruheintervall lag bei 0 %. Die Working-Set-Summe der WebView-Prozesse kann
gemeinsam genutzte Speicherseiten mehrfach zählen. Die Einstellungen sind
somit deutlich speicherintensiver als der Tray-Betrieb; in diesen fünf
Zyklen zeigte sich kein fortlaufendes Wachstum der Fensterressourcen.
Rohdaten: `artifacts/performance/app-memory.csv`.

EXE der obigen Ressourcenmessung (vor der anschließenden optischen Anpassung
der Statuskarten): `target/release/kova-screen.exe`, mit eingebautem Frontend
(`tauri/custom-protocol`). SHA-256:
`472E1799C655805B1082F8370A40E029BE8DF0BB6EED0D8E0445228A8FDEAE64`.

## Nachweise und Wiederholung

- 23 Overlay-Tests, 53 Capture-Tests, 38 Encoder-Tests und 51 App-Tests bestanden.
- Clippy über den Workspace einschließlich Test-Targets mit `-D warnings` bestanden.
- Der interaktive Ressourcen-Probe ist absichtlich vom normalen Testlauf
  ausgeschlossen und wurde separat ausgeführt.
- Lokale Rohdaten: `artifacts/performance/recording-profile-final.log`,
  `encoder-40-cycles.log`, `encoder-settled.log`, `software-encoder.log`,
  `isolated-overlay.log`, `fixed-capture.log` und `native-debug-history.log`.
  Diese generierten Dateien sind nicht versioniert.

```powershell
# In einer VS-2022-Entwickler-PowerShell:
cargo test --release --locked -p kova-screen --lib recorder::tests::profile_recording_resources -- --ignored --exact --nocapture --test-threads=1

# Isolierter Software-Encoder, optional längere Serie und Ruhephase:
$env:KOVA_PROFILE_MODE = 'encode'
$env:KOVA_PROFILE_SOFTWARE = '1'
$env:KOVA_PROFILE_CYCLES = '40'
$env:KOVA_PROFILE_SETTLE_SECONDS = '60'
cargo test --release --locked -p kova-screen --lib recorder::tests::profile_recording_resources -- --ignored --exact --nocapture --test-threads=1
```

Weitere isolierte Modi: `capture` und `overlay`. Für den vollständigen Probe
die Variablen wieder aus der Prozessumgebung entfernen. Ein begrenzter Test
kann langfristige Leaks nicht grundsätzlich ausschließen; entscheidend sind
die dokumentierten Messreihen und der verbleibende Hardware-Befund.
