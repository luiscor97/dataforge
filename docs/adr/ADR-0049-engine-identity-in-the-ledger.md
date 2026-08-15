# ADR-0049 — Qué motor produjo cada resultado

**Estado:** Propuesta
**Fecha:** 2026-08-15
**Relacionada con:** RFC-0001 §12.9 (ledger encadenado), ADR-0018 (manifiesto
congelado), ADR-0037 (contratos congelados), ADR-0043 §3 (`Actor::Agent`)

## Contexto

Durante la prueba de campo del 14–15 de agosto, el pipeline completo se ejecutó
sobre 444 GB con **tres binarios distintos**: se fueron arreglando fallos
mientras el run avanzaba, así que `analyze` y el primer plan salieron de una
compilación, `approve`/`execute`/`verify` de otra, y el paquete de entrega de
una tercera.

Cada etapa es válida por separado. Lo que no existe es forma de saberlo:

```
app_version: "1.0.1"     ← los tres escribieron esto
grep GIT_SHA|vergen|build_id → sin resultados
```

El proyecto guarda `app_version`, una cadena tomada de `CARGO_PKG_VERSION`. Las
tres compilaciones eran binarios diferentes con código diferente y las tres se
llaman igual. **El ledger no puede distinguirlas.**

Este proyecto ya se tomó en serio exactamente este problema una vez. ADR-0043 §3
introdujo `Actor::Agent` con el argumento de que *un archivo cuyo rastro de
auditoría no distingue una decisión humana de una de un modelo no es un rastro
de auditoría*. Un rastro que no distingue dos motores tampoco lo es.

No es hipotético: ocurrió, en el primer run real, sin mala intención y sin que
nada lo señalara.

## Decisión

Sellar la identidad del motor **en el `payload_json` de los eventos**, no en una
columna nueva.

El payload ya entra en `payload_hash` y de ahí en `event_hash`, así que la
identidad queda cubierta por la cadena existente sin tocar cómo se calcula. Los
eventos anteriores no cambian y siguen verificando igual. No hay migración, no
hay ruptura de bases existentes y no hay contrato congelado que mover.

La alternativa —una columna `engine` en `audit_events`— obligaría a elegir entre
dos males: incluirla en el hash, y entonces ninguna base existente verifica; o
dejarla fuera, y entonces es metadato editable sin detección, que en un ledger
es peor que no tenerlo.

La identidad se compone de:

- `version` — `CARGO_PKG_VERSION`, lo que ya se guarda.
- `commit` — SHA del árbol de fuentes, o `unknown` cuando no se puede
  determinar.
- `dirty` — si el árbol tenía cambios sin comitear al compilar.

**`unknown` y `dirty` se escriben, no se ocultan.** Un binario compilado desde
un tarball sin git no tiene commit, y un binario compilado desde un árbol sucio
no es reproducible: las dos cosas son ciertas y las dos importan para quien
audita. Rellenarlas con un valor plausible sería exactamente el fallo que este
ADR corrige.

## Alternativas consideradas

- **Columna `engine` en `audit_events`.** Descartada arriba: rompe la
  verificación de bases existentes o crea metadato no verificado.
- **Solo en el proyecto, al crearlo.** Es lo que hay hoy con `app_version`, y es
  justo lo que falló: el run duró más que el binario.
- **Solo en el paquete de entrega.** Llega tarde. El paquete lo escribe el
  último binario, que es precisamente el que no sabe cuál hasheó.
- **Hash del propio ejecutable en tiempo de arranque.** Identifica el artefacto
  exacto, que es más fuerte que un SHA de fuentes. Descartada por coste —
  leerse entero en cada arranque— y porque no dice *qué código* es sin un
  registro externo que lo traduzca.

## Consecuencias

**Positivas.** Un run reconstruido desde el ledger dice qué motor hizo cada
etapa. Un resultado producido por tres binarios queda visible como tal en vez de
parecer uno. Y `dirty: true` marca los resultados que no son reproducibles, que
es información que hoy se pierde.

**Negativas.** Cada evento crece unas decenas de bytes; en el corpus real, 38
eventos, irrelevante. Y capturar el SHA exige un `build.rs`, que es maquinaria
nueva en un workspace que hoy no tiene ninguna.

**La que hay que vigilar: builds reproducibles (M0.9).** Empotrar el SHA de
fuentes **no** rompe la reproducibilidad —mismas fuentes, mismo SHA, mismo
binario— pero empotrar la fecha o el host sí la rompería. Este ADR sella
únicamente lo que se deriva del estado de las fuentes, y la evidencia de rebuild
determinista debe volver a generarse para confirmarlo antes de aceptarlo.

**Cuándo revisar.** Si alguna vez se firma el binario (Sigstore, ADR-0039), la
identidad del artefacto firmado es más fuerte que el SHA de fuentes y este
esquema debería referirse a ella.

## Cómo se descubrió

Merece decirse porque cambia cuánto pesa. No salió de revisar el código: salió
de que quien conducía el motor cometiera el error, lo contara, y al preguntarse
si el producto lo habría detectado, comprobara que no. El defecto llevaba ahí
desde el primer día y ninguna auditoría de código lo había mirado, porque no es
un fallo — es una ausencia.
