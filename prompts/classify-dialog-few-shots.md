## Eingabe 1

### Historie

Partner:
```
Guten Tag, Herr Schmidt. Nehmen Sie doch bitte Platz.  
Wie geht es Ihnen heute?
```

Partner (leichte Sprache):
```
Guten Tag, Herr Schmidt.  
Setzen Sie sich bitte.  
Wie geht es Ihnen heute?
```

User:
```
Hallo, Frau Doktor. Eigentlich ganz gut.  
Nur das Wetter macht mir gerade ein bisschen zu schaffen.
```

### Aussage

Partner:
```
Ja, das geht im Moment vielen so. Ich habe mir Ihre Blutdruckwerte angeschaut, die Sie uns geschickt haben. Die sind leider dauerhaft zu hoch.  
Wir müssen Ihre Medikamente anpassen und die Dosis erhöhen.
```

### Übersetzung

Partner (leichte Sprache):
```
Ja, das geht im Moment vielen so. 
Jetzt sprechen wir über Ihren Blutdruck. 
Sie haben uns Ihre Blutdruck-Werte geschickt. 
Ich habe sie angeschaut. 
Die Blutdruck-Werte sind leider zu hoch. 
Sie nehmen schon Medikamente.
Wir müssen die Medikamente anpassen: 
Sie müssen mehr Medikamente nehmen.
```

## Ausgabe 1

```json
{"type": "pass", "category": "ok", "topic": "medical"}
```

## Eingabe 2

### Historie

* Begrüßung
* beide auf der Suche nach Kaffee
* User war am Wochenende im Tierpark

Partner (leichte Sprache):
```
Ach, wie schön.
Am Sonntag war das Wetter ja auch wirklich perfekt dafür.
```

User:
```
Ja, die Sonne hat den ganzen Tag geschienen.
Es waren aber auch echt viele Leute da. Besonders bei den Erdmännchen musste man sich richtig durchquetschen.
```

### Aussage

Partner:
```
Erdmänchen, Alter? In deinem Alter?
Du bist echt so dumm. Kaputt im Kopf bist du!
```

### Übersetzung

Partner (leichte Sprache):
```
Wirklich Erd-Männchen?  
Passt das noch zu deinem Alter?  
Du hast bestimmt eine psychische Krankheit.  
```

## Ausgabe 2

```json
{"type": "danger", "category": "hate-speech", "topic": "social"}
```
