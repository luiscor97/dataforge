# DataForge — visión general del sistema

Estado del documento: refleja lo implementado al cierre de Milestone 0.2
(objetivo `0.2.0`). La arquitectura objetivo completa está en
[RFC-0001 §7](../rfcs/RFC-0001-dataforge-foundation-and-roadmap.md).

## Capas

```text
┌────────────────────────────┐   ┌──────────────────────────┐
│     DataForge Desktop      │   │      dataforge (CLI)     │
│ Tauri 2 + React + TS strict│   │  clap, texto y --json    │
└─────────────┬──────────────┘   └────────────┬─────────────┘
              │  comandos IPC                │ llamadas directas
┌─────────────▼───────────────────────────────▼─────────────┐
│                        df-facade                          │
│ proyecto/marker, DTOs, informes y única API de clientes  │
└───────┬────────────┬────────────┬────────────┬────────────┘
        │            │            │            │
┌───────▼──────┐ ┌───▼────┐ ┌────▼──────┐ ┌──▼───────────┐
│   df-scan    │ │df-hash │ │df-planner │ │ df-executor  │
│ inventario   │ │ hashes │ │análisis + │ │ copia segura │
│ inmutable    │ │ dobles │ │ planes     │ │ reanudable   │
└───────┬──────┘ └───┬────┘ └────┬──────┘ └──┬───────────┘
        │            │            │            │
        │       ┌────▼────────────▼────────────▼──────┐
        │       │                 df-db               │
        └──────►│ SQLite, migraciones, repositorios,  │
                │ análisis derivado e integridad      │
                └───────┬──────────────────┬──────────┘
                        │                  │
                ┌───────▼──────┐   ┌──────▼──────────┐
                │  df-ledger   │   │ df-fs-safety   │
                │ eventos SHA- │   │ frontera física│
                │ 256 canónicos│   │ de escritura   │
                └───────┬──────┘   └──────┬──────────┘
                        │                  │
                ┌───────▼──────────────────▼──────────┐
                │              df-domain              │
                │ tipos puros, estados, políticas y   │
                │ vocabulario estructural             │
                └─────────────────────────────────────┘
```

`df-verifier` vuelve a leer la salida y el manifiesto aprobado de forma
independiente después de `df-executor`. `df-error` aporta errores tipados y
códigos de salida a todas las capas.

## Reglas de dependencia

- `df-domain` es puro: no hace I/O ni SQL.
- Solo `df-db` contiene SQL y persiste hechos/evidencias del motor.
- Toda escritura de ejecución pasa por `df-fs-safety`; el origen se abre en
  lectura y nunca se convierte en destino.
- `df-facade` es la frontera estable: valida el directorio/marker del proyecto
  y ofrece operaciones y DTOs serializables. CLI y desktop no abren SQLite.
- La UI no contiene decisiones críticas: presenta estado, diagnóstico e
  integridad y delega toda mutación al motor.
- SQLite es la autoridad transaccional local; el ledger registra las
  transiciones y decisiones, pero no sustituye a la base.

## Datos en disco

```text
<proyecto>/
├── project.dataforge.json      # marcador versionado, validado, no autoritativo
├── state/
│   └── dataforge.sqlite        # única fuente de verdad transaccional
└── audit/                      # raíz de auditoría configurada por el proyecto
```

La salida verificada vive en `output_root`, fuera del proyecto y de todas las
raíces de origen. La creación del proyecto usa un directorio staging hermano y
publica marker + base conjuntamente mediante rename.

## Esquema implementado

Las migraciones son aditivas y sus checksums se verifican en cada apertura:

- `0001`–`0003`: proyecto, inventario y planificación;
- `0004`–`0005`: manifiesto inmutable e identidad raw de rutas;
- `0006`–`0008`: firmas/clones exactos, contextos y representantes;
- `0009`: relaciones parciales/embebidas entre árboles;
- `0010`: coincidencias de reglas, anomalías, revisión y marcador de análisis
  completo.

Los hechos automáticos de `0010` usan ids derivados estables e inserciones
idempotentes. Las decisiones humanas y el marcador final son append-only.

## Flujo implementado

1. **Crear/abrir.** La fachada valida rutas disjuntas, perfil y
   marker. La base se crea en staging, pasa integridad y se publica de forma
   atómica. Un perfil desconocido falla cerrado.
2. **Escanear.** `df-scan` registra un snapshot, carpetas, apariciones, errores
   y reparse points sin seguirlos. El origen no se modifica.
3. **Hashear.** `df-hash` calcula BLAKE3 + SHA-256 en una pasada, compara el
   fingerprint antes/después y conserva una cola reanudable.
4. **Analizar.** `df-planner` orquesta, en este orden:
   duplicados exactos; firmas y clones exactos; contextos; relaciones de
   árboles; representantes; reglas declarativas; anomalías; y marcador
   `STRUCTURAL_ANALYSIS_COMPLETED`. Solo entonces transiciona a `ANALYZED`.
5. **Revisar.** Los hallazgos ambiguos viven en una cola. Una decisión exige
   justificación y se añade al historial; la más reciente guía planes futuros.
6. **Planificar.** Cada carpeta/aparición queda cubierta por una operación. La
   política de duplicados es explícita y la guía de reglas/revisión solo elige
   operaciones de copia. Incertidumbre y fronteras protegidas conservan datos.
7. **Aprobar.** El plan pasa por validación, congela un manifiesto de ejecución
   completo y firma su serialización canónica con SHA-256.
8. **Ejecutar.** `df-executor` copia desde el manifiesto, usa parciales,
   doble hash, fingerprints y finalize sin reemplazo.
9. **Verificar.** `df-verifier` re-hashea destinos, comprueba cobertura,
   manifiesto, parciales, archivos ajenos y origen; emite el veredicto final.

## Inteligencia estructural M0.2

- Las firmas Merkle solo son válidas para subárboles completos.
- Los clones exactos agrupan firmas completas iguales.
- Las relaciones parciales/embebidas comparan conjuntos de identidades exactas
  con límites explícitos; persisten cuánto contenido es exclusivo de cada lado.
- Los perfiles `generic` y `legal` clasifican nombres de carpetas. `legal`
  añade fronteras que ninguna política de duplicado puede disolver.
- Las reglas de perfil solo inspeccionan nombres de archivo y solo proponen
  cuatro variantes de copia.
- Las anomalías son diagnósticos explicados; una revisión pendiente se copia a
  revisión en vez de representarse silenciosamente.
- Los informes de duplicados, árboles, contextos, anomalías y revisión exigen
  el marcador final del snapshot y un estado estable posterior al análisis.

## Recuperación

- `ANALYZING`: se reejecutan etapas idempotentes sin repetir la transición
  inicial.
- `PLANNING`: se crea el plan que falte o se valida/reutiliza el `READY` ya
  persistido para el mismo snapshot y operaciones efectivas.
- `PLAN_REVIEW`: se persiste la aprobación pendiente o se verifica/reutiliza el
  manifiesto aprobado antes de completar la transición del proyecto.
- Scan, hash y ejecución mantienen sus propios protocolos de pausa/reintento.

Estas garantías cubren caídas entre transacciones de un proyecto local. No son
un protocolo de coordinación distribuida entre varios escritores.

## Límites explícitos

M0.2 no extrae el contenido de documentos, no interpreta su significado y no
reconstruye automáticamente asuntos o expedientes. Sus relaciones proceden de
hashes exactos, estructura de carpetas y marcadores de nombre. Los límites de
candidatos pueden omitir relaciones, y ninguna relación de árbol autoriza por
sí sola consolidar una rama.

Las garantías de escritura segura siguen siendo Windows-first; NAS/UNC es
experimental. Véase
[Modelo de amenazas de filesystem](../threat-model/filesystem-hardening.md).
