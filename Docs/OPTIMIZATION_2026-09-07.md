# Aufnahme- und Ressourcenverbesserungen

## Umgesetzte Änderungen

- Eine gemeinsame Aufnahmeuhr steuert Timer, Ausgabe-Zeitstempel und Zeitlimit.
  Pausen werden herausgerechnet, einschließlich einer beim Stop noch aktiven
  Pause. Die Capture-Schleife überspringt weiterhin die GPU-Rücklesearbeit.
- Ein erschöpftes GIF-Größenbudget beendet die Capture-Schleife regulär.
  Der vorhandene Watcher schließt die Datei, entfernt die Overlays und meldet
  die gespeicherte, am Limit gekürzte Aufnahme.
- Bereichs-Screenshots wenden die konfigurierte Verzögerung an und zeichnen
  bei aktivierter Cursoroption den ursprünglichen Mauszeiger in den eingefrorenen
  Desktop. Die ausgewählten Pixel stammen weiterhin aus diesem Snapshot.
- Ein Auswahl-/Startvorgang lässt sich nicht durch schnelle Capture-Tastendrücke
  mehrfach gleichzeitig öffnen. Stop bleibt erreichbar.
- Automatische Uploads verwenden einen ruhenden Worker mit maximal acht
  wartenden Jobs statt eines neuen Threads pro Datei. Bei voller oder nicht
  verfügbarer Warteschlange wird der Nutzer auf die lokal gespeicherte Datei
  und den manuellen Wiederholungsweg im Verlauf hingewiesen.
- Manuelle Uploads und Online-Löschanfragen laufen außerhalb des UI-Eventloops.
  Die Verlaufsliste zeigt „Uploading…“ und deaktiviert währenddessen die
  Zeilenaktionen, um konkurrierende Änderungen zu vermeiden.
- Neue Einstellungen verwenden Software-Encoding. Der optionale Hardwarepfad
  bleibt ausdrücklich experimentell; bestehende Präferenzen werden nicht
  stillschweigend überschrieben. Die lokale Testinstallation verwendet weiterhin
  Software-Encoding.

## Validierung

- 316 Workspace-Tests bestanden, anschließend die finalen 53 App-Tests erneut.
- Der Pausentest prüft eine Pause von 1,2 Sekunden bei einem Aufnahmelimit von
  einer Sekunde: Die Aufnahme bleibt aktiv, der Timer steht, und die Pause
  erscheint nicht in der gemeldeten Dauer.
- Der neue GIF-Integrationstest erzwingt ein sehr kleines Budget, prüft das
  selbstständige Ende nach einem Frame und den GIF-Abschlussmarker.
- Ein deterministischer Uhrentest prüft wiederholte Pause-Aufrufe, Fortsetzung
  und Stop während einer noch aktiven Pause.
- Workspace-Clippy mit `-D warnings` und TypeScript-/Vite-Build bestanden.
- Acht MP4- und acht GIF-Aufnahmen mit echten Desktop-Frames, jeweils zwei
  Sekunden bei 1280×720, bestanden. Nach der abschließenden Ruhephase:
  47,28 MiB Working Set, 29,91 MiB Private Bytes, 495 Handles,
  vier GDI- und zwei USER-Objekte. Die letzte GIF-Serie blieb nach dem
  Schließen bei 495 Handles; kein Wachstum der Overlay-Ressourcen.
- Lokale Testlogs: `artifacts/performance/optimization-tests.log`,
  `optimization-app-final.log` und `optimization-resources.log`.

## Verbleibende Grenze

Der zuvor nachgewiesene native Handle-Zuwachs bei Hardware-Encoding ist nicht
an seiner Treiber-/Runtime-Ursache behoben. Software als Standard vermeidet
diesen Pfad; es ist keine Reparatur des Hardware-Encoders. Die frühere
Software-Serie mit 40 Dateien und Ruhephase zeigte verzögerte Speicherfreigabe
und ist in `RECHECK_2026-09-07.md` dokumentiert. Eine Zusicherung, dass auf jeder
Grafikkarte und über beliebig lange Laufzeiten keine Leaks auftreten, lässt
sich daraus nicht ableiten.

Echte externe Uploads und Online-Löschungen wurden nicht ausgelöst; deren
Erfolg mit dem Benutzerkonto ist damit nicht bestätigt.
