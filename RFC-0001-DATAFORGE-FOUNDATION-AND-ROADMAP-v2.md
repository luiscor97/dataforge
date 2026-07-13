# DataForge
## RFC-0001 — Documento fundacional, arquitectura y roadmap maestro

**Estado:** Aprobado para iniciar implementación  
**Versión del documento:** 1.0.0  
**Fecha:** 2026-07-13  
**Autor del proyecto:** Luis Cordero / Luiscor IT Services  
**Naturaleza:** proyecto open source local-first  
**Plataforma inicial:** Windows 10/11 x64  
**Lenguaje principal:** Rust  
**Interfaz:** Tauri 2 + React + TypeScript  
**Licencia propuesta:** Apache-2.0 OR MIT  
**Nombre de trabajo:** DataForge  
**Repositorio sugerido:** `dataforge`  
**Ejecutable CLI:** `dataforge`  
**Aplicación de escritorio:** `DataForge Desktop`

---

# 0. Mandato para Codex

Este documento es la especificación fundacional de DataForge.

Codex debe leerlo completo antes de crear o modificar código. Su función no es improvisar una aplicación visual ni reproducir scripts aislados del caso de Asesoría Jurídica. Debe implementar un motor general de reconstrucción documental que sea:

- seguro;
- local;
- verificable;
- reproducible;
- modular;
- extensible;
- auditable;
- eficiente;
- comprensible para usuarios no técnicos;
- útil sin inteligencia artificial;
- abierto a contribuciones de terceros.

## Reglas absolutas

1. El origen es inmutable.
2. El MVP no borra archivos.
3. El MVP no sobrescribe archivos.
4. Todo resultado debe derivar de un snapshot identificable.
5. SQLite es la única fuente de verdad transaccional.
6. Los CSV, JSON, Markdown y PDF son exportaciones.
7. Ruta física y contenido son entidades distintas.
8. Un duplicado exacto no es automáticamente prescindible.
9. Contextos protegidos prevalecen sobre deduplicación agresiva.
10. Toda ejecución parte de un plan aprobado e inmutable.
11. Toda operación tiene una clave de idempotencia.
12. Toda copia se verifica criptográficamente.
13. Toda interrupción debe ser recuperable.
14. La IA no ejecuta acciones sobre archivos.
15. Los plugins no reciben acceso ambiental por defecto.
16. La interfaz no contiene la lógica crítica.
17. El motor debe poder utilizarse sin la interfaz.
18. Cada hito debe terminar compilando y con pruebas.
19. No se afirma que una función funciona si no hay evidencia de prueba.
20. Ninguna decisión de seguridad se reduce para acelerar el desarrollo.

---


# 0.1. Preparación autónoma del entorno, herramientas, plugins y skills

Codex está autorizado y obligado a preparar de forma autónoma el entorno necesario para desarrollar, probar, auditar, empaquetar y documentar DataForge.

Antes de comenzar cualquier implementación debe:

1. Inspeccionar el sistema y el repositorio.
2. Detectar qué herramientas, runtimes, gestores de paquetes, plugins, extensiones y skills ya están disponibles.
3. Identificar qué elementos faltan para completar el hito actual.
4. Instalar y configurar de forma autónoma únicamente los elementos necesarios.
5. Verificar versiones, compatibilidad y funcionamiento.
6. Documentar todo lo instalado, configurado o descartado.
7. Continuar el trabajo sin detenerse por dependencias que pueda resolver de forma segura.

## 0.1.1. Herramientas que puede instalar o configurar

Codex puede instalar, actualizar o configurar, cuando sean necesarias:

- Git;
- GitHub CLI;
- Rust mediante `rustup`;
- toolchain estable de Rust;
- componentes `rustfmt` y `clippy`;
- targets adicionales de compilación;
- Cargo y utilidades del ecosistema Rust;
- Node.js LTS;
- Corepack;
- pnpm;
- Tauri CLI;
- dependencias nativas requeridas por Tauri;
- SQLite y herramientas de inspección;
- CMake;
- Ninja;
- LLVM/Clang;
- Visual Studio Build Tools;
- WebView2 y dependencias de Windows necesarias;
- herramientas de testing;
- herramientas de benchmarking;
- herramientas de fuzzing;
- herramientas de auditoría de dependencias;
- generadores de documentación;
- herramientas de empaquetado;
- herramientas de firma, cuando existan credenciales disponibles;
- utilidades auxiliares necesarias para compilar, probar, verificar o distribuir DataForge.

## 0.1.2. Plugins, extensiones y skills

Codex debe evaluar e instalar o habilitar de forma autónoma los plugins, extensiones y skills que mejoren de forma material el desarrollo del proyecto.

Puede incluir, entre otros:

- skills oficiales o mantenidos para Rust;
- skills para Tauri;
- skills para React y TypeScript;
- skills para SQLite;
- skills de seguridad;
- skills de revisión de código;
- skills de testing;
- skills de documentación;
- skills de arquitectura;
- skills de análisis de dependencias;
- skills de CI/CD;
- plugins de GitHub;
- extensiones del editor;
- herramientas MCP;
- servidores de lenguaje;
- analizadores estáticos;
- linters;
- formatters;
- generadores de esquemas;
- asistentes para migraciones;
- herramientas de observabilidad;
- herramientas de inspección de binarios;
- plugins de benchmark y profiling.

## 0.1.3. Criterios para instalar un plugin o skill

Un plugin o skill solo debe instalarse cuando:

1. Resuelva una necesidad concreta del hito actual.
2. Sea compatible con el stack y las versiones utilizadas.
3. Tenga una fuente identificable y confiable.
4. No requiera entregar acceso innecesario al sistema o a datos personales.
5. No introduzca ejecución remota opaca.
6. No duplique una capacidad ya disponible.
7. No degrade la reproducibilidad del entorno.
8. Pueda documentarse y verificarse.
9. Su licencia sea compatible con el proyecto.
10. Su mantenimiento y procedencia sean razonablemente fiables.

## 0.1.4. Fuentes permitidas

Preferir siempre:

- documentación oficial;
- repositorios oficiales;
- registros oficiales de paquetes;
- `rustup`;
- `cargo install`;
- `winget`;
- `corepack`;
- `pnpm`;
- `npm`;
- releases oficiales de GitHub verificables;
- extensiones publicadas por el proveedor oficial;
- skills incluidos oficialmente en el entorno;
- plugins mantenidos por proyectos reconocidos;
- MCPs con código y permisos revisables.

No instalar desde:

- URLs anónimas;
- scripts sin revisar;
- binarios sin procedencia;
- repositorios clonados sin comprobar;
- paquetes con nombres sospechosos o typosquatting;
- fuentes que soliciten credenciales no necesarias;
- plugins que requieran acceso global al sistema sin justificación.

## 0.1.5. Prohibición de ejecución remota opaca

No ejecutar directamente patrones como:

```text
curl <url> | sh
wget -qO- <url> | bash
irm <url> | iex
iwr <url> | powershell
```

salvo que:

1. la fuente sea oficial;
2. el contenido se descargue primero;
3. el contenido se inspeccione;
4. se verifique su integridad cuando exista checksum o firma;
5. no exista una alternativa más segura;
6. la decisión quede documentada.

## 0.1.6. Privilegios y cambios del sistema

Codex puede realizar instalaciones en espacio de usuario sin solicitar confirmación adicional.

Debe evitar privilegios de administrador salvo que sean imprescindibles.

Si una instalación requiere:

- elevación UAC;
- reinicio;
- aceptación manual de una licencia;
- credenciales;
- acceso a una cuenta;
- modificación de políticas de seguridad;
- apertura de puertos;
- desactivación del antivirus;
- instalación de drivers;
- modificación del registro fuera de lo necesario;
- cambios globales que afecten a otros proyectos;

debe:

1. detener únicamente esa instalación;
2. documentar el bloqueo;
3. proponer la alternativa segura;
4. continuar con las tareas no bloqueadas.

Codex no debe:

- desactivar controles de seguridad;
- desinstalar toolchains existentes sin causa demostrada;
- modificar archivos personales ajenos al repositorio;
- cambiar el shell predeterminado;
- cambiar políticas corporativas;
- instalar software de procedencia dudosa;
- limpiar configuraciones globales;
- eliminar plugins o skills existentes sin evaluar su impacto;
- habilitar telemetría sin consentimiento;
- guardar secretos en texto plano.

## 0.1.7. Skills del proyecto

Codex debe crear y mantener una capa de skills propia del repositorio cuando detecte tareas repetitivas o decisiones que deban estandarizarse.

Ubicación sugerida:

```text
.codex/
├── skills/
│   ├── bootstrap-environment/
│   ├── rust-quality-gate/
│   ├── tauri-build/
│   ├── sqlite-migrations/
│   ├── security-review/
│   ├── release-check/
│   └── dataforge-invariants/
├── prompts/
└── config/
```

Cada skill propia debe incluir:

```text
nombre
objetivo
cuándo usarla
entradas
salidas
herramientas permitidas
límites
comandos
criterios de éxito
fallos esperados
```

Las skills del repositorio no deben:

- otorgar acceso libre al shell a un LLM externo;
- modificar el origen de proyectos DataForge;
- saltarse tests;
- ocultar errores;
- ejecutar acciones destructivas;
- sustituir decisiones de seguridad.

## 0.1.8. Plugins del proyecto

Codex debe diferenciar:

### Plugins de desarrollo

Usados para construir DataForge:

- linters;
- analizadores;
- extensiones;
- asistentes de CI;
- MCPs;
- herramientas de revisión.

### Plugins de DataForge

Extensiones que formarán parte del producto:

- extractores;
- clasificadores;
- rule packs;
- analizadores de similitud;
- reporters;
- conectores;
- perfiles.

Los plugins de desarrollo pueden instalarse durante el bootstrap.

Los plugins de DataForge deben seguir el roadmap y no deben implementarse antes del Milestone 0.6 salvo stubs de interfaz estrictamente necesarios.

## 0.1.9. Bootstrap reproducible

Codex debe crear y mantener:

```text
scripts/bootstrap-windows.ps1
scripts/check-environment.ps1
scripts/install-dev-tools.ps1
scripts/install-dev-plugins.ps1
scripts/verify-toolchain.ps1
docs/contributor-guide/environment-setup.md
docs/contributor-guide/plugins-and-skills.md
docs/environment-report.md
```

Los scripts deben ser:

- idempotentes;
- seguros;
- legibles;
- comentados;
- capaces de detectar instalaciones existentes;
- capaces de instalar solo lo que falte;
- capaces de verificar el resultado;
- capaces de devolver códigos de error útiles;
- aptos para un equipo Windows limpio;
- compatibles con ejecución repetida.

## 0.1.10. Informe de entorno

Antes de desarrollar, Codex debe generar:

```text
docs/environment-report.md
```

Contenido mínimo:

- sistema operativo;
- arquitectura;
- shell;
- versiones detectadas;
- herramientas preexistentes;
- herramientas instaladas;
- plugins instalados;
- skills disponibles;
- skills creadas para el repositorio;
- comandos utilizados;
- fuentes de instalación;
- checksums o firmas cuando existan;
- licencias relevantes;
- variables de entorno;
- incidencias;
- bloqueos;
- acciones manuales pendientes.

## 0.1.11. Registro de decisiones

Toda instalación no trivial debe quedar documentada en:

```text
docs/adr/
```

Ejemplos:

```text
ADR-0011-rust-toolchain-version.md
ADR-0012-node-and-pnpm-policy.md
ADR-0013-development-plugins.md
ADR-0014-codex-skills-policy.md
```

## 0.1.12. Regla de autonomía

Codex no debe detener el proyecto para preguntar por una dependencia que pueda:

- detectar;
- instalar;
- configurar;
- verificar;
- sustituir por una alternativa segura.

Debe resolverla y continuar.

Solo debe marcar una tarea como bloqueada cuando exista una limitación real de:

- permisos;
- hardware;
- licencia;
- credenciales;
- acceso a cuenta;
- política corporativa;
- incompatibilidad técnica;
- riesgo de seguridad no resoluble.

En ese caso debe continuar con todas las tareas independientes.


# 1. Resumen ejecutivo

DataForge es una plataforma local de inteligencia, reconstrucción y migración documental.

Su propósito es transformar discos, carpetas compartidas, copias de seguridad, NAS y archivos históricos desordenados en estructuras utilizables sin destruir el origen y conservando la trazabilidad de cada decisión.

No es solamente:

- un detector de duplicados;
- un buscador de archivos grandes;
- un explorador de disco;
- un limpiador;
- una interfaz para un LLM;
- un gestor documental tradicional.

DataForge combina:

1. inventario físico;
2. identidad criptográfica;
3. identidad parcial por chunks;
4. similitud textual;
5. similitud multimedia;
6. reconstrucción de contextos;
7. detección de árboles injertados;
8. asociación de correos y adjuntos;
9. planificación verificable;
10. copia transaccional;
11. auditoría criptográfica;
12. revisión humana;
13. plugins aislados;
14. IA opcional y subordinada.

## Propuesta de valor

> DataForge analiza un archivo digital caótico, reconstruye sus relaciones, propone una organización justificable y crea una salida verificada sin modificar el origen.

## Promesa mínima

El usuario debe poder afirmar:

> “DataForge ha analizado mi carpeta sin tocarla, ha identificado contenidos, duplicados, versiones y anomalías, me ha mostrado un plan, he aprobado las decisiones y ha creado una copia ordenada con verificación e informe.”

---

# 2. Origen y validación del problema

DataForge nace de un caso real:

- un origen con más de 150.000 archivos;
- cientos de gigabytes;
- expedientes jurídicos;
- correos;
- periciales;
- audio y vídeo;
- carpetas injertadas;
- descargas;
- escritorios recuperados;
- duplicados exactos;
- nombres repetidos con contenidos distintos;
- temporales;
- recursos formativos;
- material no jurídico;
- ausencia de una estructura fiable.

El procedimiento artesanal demostró que el problema es resoluble, pero también mostró sus límites:

- reglas dispersas;
- sucesión de scripts;
- informes intermedios contradictorios;
- ausencia de código reutilizable;
- ausencia de una máquina de estados;
- ausencia de pruebas de regresión;
- dificultad para diferenciar contenido operativo y contenedor físico.

DataForge convierte ese método en producto.

---

# 3. Alcance estratégico

## 3.1. Visión

Crear el estándar open source para:

- inventariar;
- comprender;
- reconstruir;
- migrar;
- verificar;
- documentar

archivos digitales desordenados.

## 3.2. Público inicial

### Profesionales y organizaciones

- despachos jurídicos;
- asesorías;
- gestorías;
- asociaciones;
- colegios profesionales;
- administradores de fincas;
- pymes;
- departamentos administrativos;
- técnicos de sistemas;
- consultores de migración;
- archivos históricos internos.

### Usuarios técnicos

- administradores de sistemas;
- especialistas en datos;
- consultores;
- responsables de cumplimiento;
- analistas forenses no destructivos;
- desarrolladores de gestores documentales.

### Público posterior

- fotógrafos;
- productoras;
- investigadores;
- archivos familiares;
- usuarios con múltiples discos;
- organizaciones culturales.

## 3.3. Casos de uso

1. Migrar un servidor histórico.
2. Reconstruir un disco heredado.
3. Ordenar una carpeta compartida de años.
4. Analizar una copia de seguridad.
5. Unificar archivos dispersos.
6. Localizar duplicados exactos.
7. Encontrar versiones de documentos.
8. Reconstruir hilos de correo.
9. Detectar árboles completos copiados dentro de otros.
10. Separar contenido operativo de material no relacionado.
11. Preparar una salida para SharePoint, OneDrive o un DMS.
12. Crear un informe auditable de una migración.
13. Preservar documentos periciales sin fusionarlos por nombre.
14. Identificar contenidos presentes solo dentro de archivos comprimidos.
15. Buscar documentación mediante contenido y contexto.

---

# 4. No objetivos

DataForge no será inicialmente:

- una herramienta de borrado masivo;
- un antivirus;
- una herramienta de recuperación de archivos eliminados;
- un sistema de e-discovery certificado;
- un DMS completo;
- un sustituto de SharePoint;
- un motor de OCR como producto principal;
- una plataforma cloud que obligue a subir archivos;
- un agente autónomo con shell;
- una aplicación móvil;
- un sincronizador bidireccional;
- un sistema de backup;
- una herramienta de modificación de documentos;
- una plataforma de firma electrónica;
- un motor jurídico que emita conclusiones legales.

---

# 5. Principios de producto

## 5.1. Local-first

El análisis se realiza localmente.

No se requiere:

- cuenta;
- login;
- conexión a Internet;
- suscripción;
- API;
- nube.

## 5.2. Safe-by-default

Las opciones más seguras son las predeterminadas.

Ejemplos:

- no seguir enlaces;
- no borrar;
- no sobrescribir;
- no enviar contenido a un modelo externo;
- no colapsar contextos;
- copiar en lugar de mover.

## 5.3. Explainable-by-design

Cada propuesta contiene:

- regla;
- evidencia;
- confianza;
- riesgo;
- contexto;
- consecuencia;
- versión del algoritmo.

## 5.4. Human-in-command

El usuario controla:

- perfiles;
- reglas;
- revisión;
- aprobaciones;
- ejecución;
- exportación;
- uso de IA.

## 5.5. Engine-first

El producto principal es `dataforge-core`.

La aplicación de escritorio es un cliente del motor.

## 5.6. Deterministic-first

Primero:

- metadatos;
- hashes;
- reglas;
- grafos;
- extractores;
- algoritmos reproducibles.

Después:

- modelos estadísticos;
- embeddings;
- LLM.

## 5.7. Open formats

Usar formatos documentados:

- SQLite;
- JSON;
- CSV;
- JSONL;
- Parquet;
- Markdown;
- PDF;
- WIT para plugins.

## 5.8. Stable contracts

Las APIs internas, esquemas y formatos exportados deben versionarse.

---

# 6. Decisiones arquitectónicas iniciales

## ADR-0001 — Rust como núcleo

**Decisión:** implementar el motor en Rust.

**Motivos:**

- rendimiento;
- seguridad de memoria;
- control de E/S;
- concurrencia;
- ejecutables autocontenidos;
- integración con Tauri;
- ecosistema de búsqueda y analítica;
- WASI/Wasmtime;
- portabilidad futura.

Python puede usarse en:

- prototipos;
- notebooks de investigación;
- generación de fixtures;
- comparación de algoritmos.

No será una dependencia necesaria del producto final.

## ADR-0002 — Tauri 2 para escritorio

**Decisión:** Tauri 2 + React + TypeScript strict.

La interfaz se comunica con una fachada controlada del motor.

No debe acceder directamente al sistema de archivos.

## ADR-0003 — SQLite como estado transaccional

SQLite almacena:

- proyectos;
- snapshots;
- rutas;
- contenidos;
- clasificaciones;
- planes;
- aprobaciones;
- operaciones;
- resultados;
- auditoría.

## ADR-0004 — Parquet y Arrow para analítica

Los snapshots grandes pueden materializarse en Parquet.

Arrow representa datos columnares.

DataFusion ejecuta consultas analíticas embebidas.

## ADR-0005 — Tantivy para búsqueda de texto

Tantivy indexa:

- nombres;
- rutas;
- texto extraído;
- metadatos;
- entidades;
- asuntos de correo.

## ADR-0006 — Wasmtime/WASI para plugins

Plugins de terceros se ejecutan con capacidades explícitas.

No se cargan DLL arbitrarias dentro del proceso principal.

## ADR-0007 — SHA-256 canónico y BLAKE3 operativo

- SHA-256: identidad de auditoría e interoperabilidad.
- BLAKE3: caché, rendimiento, árboles y chunks.

## ADR-0008 — FastCDC para similitud parcial

FastCDC se utilizará en una fase posterior para:

- chunks definidos por contenido;
- versiones;
- contenido reutilizado;
- archivos parcialmente relacionados.

## ADR-0009 — Sin grafo externo obligatorio

El grafo contextual se implementará inicialmente sobre SQLite y estructuras en memoria.

No introducir Neo4j, ArangoDB u otro servicio externo.

## ADR-0010 — Sin IA en el núcleo 0.1

La primera release funcional no necesita IA.

---

# 7. Arquitectura general

```text
┌─────────────────────────────────────────────┐
│              DataForge Desktop              │
│        Tauri 2 + React + TypeScript         │
└──────────────────────┬──────────────────────┘
                       │ Commands / Events
┌──────────────────────▼──────────────────────┐
│              DataForge Facade               │
│ API segura y estable para clientes locales  │
└──────────────────────┬──────────────────────┘
                       │
┌──────────────────────▼──────────────────────┐
│              DataForge Core                 │
│                                             │
│ Scan │ Hash │ Context │ Rules │ Plan        │
│ Copy │ Verify │ Ledger │ Report │ Search    │
└──────┬─────────┬────────┬────────┬──────────┘
       │         │        │        │
       ▼         ▼        ▼        ▼
   SQLite     Parquet   Tantivy   Plugin Host
                │                   │
                ▼                   ▼
            DataFusion        Wasmtime/WASI
```

## Procesos

### Proceso principal

- interfaz;
- coordinación;
- estado;
- comandos;
- eventos.

### Workers internos

- escaneo;
- hashing;
- extracción;
- indexación;
- chunks;
- copia;
- verificación.

No crear procesos separados hasta que exista una necesidad medible.

---

# 8. Estructura del repositorio

```text
dataforge/
├── .github/
│   ├── ISSUE_TEMPLATE/
│   ├── workflows/
│   ├── CODEOWNERS
│   ├── dependabot.yml
│   └── pull_request_template.md
│
├── apps/
│   ├── desktop/
│   │   ├── src/
│   │   ├── src-tauri/
│   │   ├── e2e/
│   │   └── package.json
│   │
│   ├── cli/
│   │   └── src/
│   │
│   └── daemon/
│       └── README.md
│
├── crates/
│   ├── df-domain/
│   ├── df-error/
│   ├── df-config/
│   ├── df-db/
│   ├── df-ledger/
│   ├── df-scan/
│   ├── df-hash/
│   ├── df-content/
│   ├── df-chunk/
│   ├── df-context/
│   ├── df-graph/
│   ├── df-rules/
│   ├── df-anomaly/
│   ├── df-similarity/
│   ├── df-extract/
│   ├── df-mail/
│   ├── df-image/
│   ├── df-audio/
│   ├── df-video/
│   ├── df-archive/
│   ├── df-planner/
│   ├── df-executor/
│   ├── df-verifier/
│   ├── df-query/
│   ├── df-search/
│   ├── df-report/
│   ├── df-plugin-api/
│   ├── df-plugin-host/
│   ├── df-ai/
│   └── df-facade/
│
├── sdk/
│   ├── wit/
│   ├── rust/
│   ├── typescript/
│   └── examples/
│
├── profiles/
│   ├── generic/
│   ├── legal/
│   ├── business/
│   ├── photography/
│   └── migration/
│
├── schemas/
│   ├── project/
│   ├── plan/
│   ├── report/
│   ├── plugin/
│   └── ai/
│
├── fixtures/
│   ├── synthetic-small/
│   ├── legal-anonymized/
│   ├── interruption/
│   ├── unicode/
│   └── generated-large/
│
├── benchmarks/
│   ├── scan/
│   ├── hash/
│   ├── chunk/
│   ├── search/
│   └── copy/
│
├── fuzz/
│   ├── path-parser/
│   ├── plan-validator/
│   ├── archive-reader/
│   └── plugin-protocol/
│
├── docs/
│   ├── rfcs/
│   ├── adr/
│   ├── architecture/
│   ├── algorithms/
│   ├── threat-model/
│   ├── user-guide/
│   ├── contributor-guide/
│   └── release/
│
├── scripts/
├── tools/
├── website/
├── Cargo.toml
├── Cargo.lock
├── package.json
├── pnpm-workspace.yaml
├── rust-toolchain.toml
├── deny.toml
├── LICENSE-APACHE
├── LICENSE-MIT
├── README.md
├── CONTRIBUTING.md
├── CODE_OF_CONDUCT.md
├── SECURITY.md
├── GOVERNANCE.md
└── CHANGELOG.md
```

---

# 9. Modelo de dominio

## 9.1. `Project`

```rust
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub state: ProjectState,
    pub profile: ProfileRef,
    pub source_roots: Vec<SourceRootId>,
    pub output_root: PathBuf,
    pub audit_root: PathBuf,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub app_version: Version,
}
```

## 9.2. `SourceRoot`

```rust
pub struct SourceRoot {
    pub id: SourceRootId,
    pub project_id: ProjectId,
    pub absolute_path: PathBuf,
    pub volume_id: Option<String>,
    pub filesystem: FileSystemKind,
    pub is_network: bool,
    pub is_removable: bool,
    pub read_only_policy: bool,
}
```

## 9.3. `PathOccurrence`

Representa una aparición física.

```rust
pub struct PathOccurrence {
    pub id: OccurrenceId,
    pub source_root_id: SourceRootId,
    pub absolute_path: PathBuf,
    pub relative_path: PathBuf,
    pub parent_relative_path: PathBuf,
    pub file_name: OsString,
    pub normalized_name: String,
    pub extension: Option<String>,
    pub size_bytes: u64,
    pub created_at_fs: Option<Timestamp>,
    pub modified_at_fs: Option<Timestamp>,
    pub attributes: FileAttributes,
    pub path_length: u32,
    pub depth: u32,
    pub file_identity: Option<FileIdentity>,
    pub fingerprint: SnapshotFingerprint,
    pub scan_status: ScanStatus,
}
```

## 9.4. `ContentObject`

Representa contenido binario único.

```rust
pub struct ContentObject {
    pub id: ContentId,
    pub size_bytes: u64,
    pub sha256: Option<Sha256Digest>,
    pub blake3: Option<Blake3Digest>,
    pub mime_type: Option<String>,
    pub first_seen_snapshot: SnapshotId,
    pub hash_state: HashState,
}
```

## 9.5. `ContentChunk`

```rust
pub struct ContentChunk {
    pub id: ChunkId,
    pub content_id: ContentId,
    pub offset: u64,
    pub length: u32,
    pub blake3: Blake3Digest,
    pub algorithm_version: String,
}
```

## 9.6. `ContextNode`

```rust
pub struct ContextNode {
    pub id: ContextId,
    pub project_id: ProjectId,
    pub parent_id: Option<ContextId>,
    pub label: String,
    pub normalized_label: String,
    pub kind: ContextKind,
    pub protected_boundary: bool,
    pub confidence: Confidence,
}
```

## 9.7. `Relationship`

```rust
pub struct Relationship {
    pub id: RelationshipId,
    pub from: SubjectRef,
    pub to: SubjectRef,
    pub kind: RelationshipKind,
    pub confidence: Confidence,
    pub evidence: Vec<Evidence>,
    pub source: DecisionSource,
}
```

## 9.8. `Classification`

```rust
pub struct Classification {
    pub id: ClassificationId,
    pub subject: SubjectRef,
    pub category: String,
    pub subcategory: Option<String>,
    pub confidence: Confidence,
    pub risk: RiskLevel,
    pub source: DecisionSource,
    pub evidence: Vec<Evidence>,
    pub ruleset_version: Version,
}
```

## 9.9. `Plan`

```rust
pub struct Plan {
    pub id: PlanId,
    pub project_id: ProjectId,
    pub snapshot_id: SnapshotId,
    pub version: u32,
    pub status: PlanStatus,
    pub serialized_sha256: Option<Sha256Digest>,
    pub created_at: Timestamp,
    pub approved_at: Option<Timestamp>,
}
```

## 9.10. `PlanOperation`

```rust
pub struct PlanOperation {
    pub id: OperationId,
    pub plan_id: PlanId,
    pub sequence: u64,
    pub operation_type: OperationType,
    pub source_occurrence: Option<OccurrenceId>,
    pub content_id: Option<ContentId>,
    pub destination_relative_path: Option<PathBuf>,
    pub confidence: Confidence,
    pub risk: RiskLevel,
    pub approval: ApprovalState,
    pub execution_state: ExecutionState,
    pub idempotency_key: String,
    pub reason: String,
}
```

---

# 10. Esquema SQLite inicial

## 10.1. Tablas obligatorias en 0.1

```text
schema_migrations
projects
source_roots
snapshots
scan_runs
folders
path_occurrences
content_objects
occurrence_content
hash_jobs
contexts
context_memberships
classifications
relationships
duplicate_sets
plans
plan_operations
operation_results
verification_runs
verification_findings
audit_events
project_metrics
```

## 10.2. Tablas posteriores

```text
chunks
chunk_memberships
document_representations
text_segments
entities
entity_mentions
mail_messages
mail_threads
mail_attachments
image_fingerprints
audio_fingerprints
video_fingerprints
archive_entries
search_indexes
plugin_registry
plugin_runs
ai_requests
ai_responses
manual_rules
```

## 10.3. Reglas de SQLite

- foreign keys activadas;
- migraciones versionadas;
- transacciones;
- WAL evaluado mediante pruebas;
- índices explícitos;
- no almacenar archivos binarios;
- no almacenar texto completo innecesario en tablas transaccionales;
- eventos append-only;
- cada tabla con `created_at`;
- campos de versión cuando proceda.

---

# 11. Máquina de estados

```text
CREATED
VALIDATING
READY
SCANNING
SCAN_PAUSED
SCANNED
HASHING
HASH_PAUSED
HASHED
ANALYZING
ANALYSIS_PAUSED
ANALYZED
PLANNING
PLAN_READY
PLAN_REVIEW
PLAN_APPROVED
EXECUTING
EXECUTION_PAUSED
EXECUTED
VERIFYING
COMPLETED
COMPLETED_WITH_WARNINGS
FAILED
ARCHIVED
```

## Transiciones permitidas

```text
CREATED → VALIDATING
VALIDATING → READY | FAILED
READY → SCANNING
SCANNING → SCAN_PAUSED | SCANNED | FAILED
SCAN_PAUSED → SCANNING | FAILED
SCANNED → HASHING
HASHING → HASH_PAUSED | HASHED | FAILED
HASH_PAUSED → HASHING | FAILED
HASHED → ANALYZING
ANALYZING → ANALYSIS_PAUSED | ANALYZED | FAILED
ANALYSIS_PAUSED → ANALYZING | FAILED
ANALYZED → PLANNING
PLANNING → PLAN_READY | FAILED
PLAN_READY → PLAN_REVIEW
PLAN_REVIEW → PLAN_REVIEW | PLAN_APPROVED
PLAN_APPROVED → EXECUTING
EXECUTING → EXECUTION_PAUSED | EXECUTED | FAILED
EXECUTION_PAUSED → EXECUTING | FAILED
EXECUTED → VERIFYING
VERIFYING → COMPLETED | COMPLETED_WITH_WARNINGS | FAILED
```

## Invariantes

- un plan aprobado es inmutable;
- una operación completada no vuelve a ejecutarse;
- un proyecto completo tiene `verification_id`;
- un estado solo cambia mediante un comando de dominio;
- cada cambio genera evento;
- no saltar fases mediante UI.

---

# 12. Pipeline funcional

```text
VALIDATE
→ SNAPSHOT
→ HASH
→ CONTEXT
→ RULES
→ RELATIONSHIPS
→ REVIEW
→ PLAN
→ APPROVE
→ EXECUTE
→ VERIFY
→ REPORT
```

## 12.1. Validación

Comprobar:

- origen;
- salida;
- auditoría;
- espacio;
- permisos;
- solapamiento;
- filesystem;
- red;
- enlaces;
- placeholders cloud;
- cifrado;
- nombres ilegales;
- rutas extremas.

## 12.2. Snapshot

Registrar:

- árbol;
- metadatos;
- identificadores físicos;
- fingerprint;
- errores;
- firma agregada;
- límites.

## 12.3. Hash

- BLAKE3 operativo;
- SHA-256 canónico;
- validación pre/post;
- caché;
- trabajos reanudables.

## 12.4. Contexto

- carpetas;
- entidades;
- límites;
- áreas protegidas;
- marcadores genéricos;
- grafo.

## 12.5. Reglas

- tipos;
- temporales;
- software;
- correo;
- multimedia;
- rutas;
- nombres;
- perfiles.

## 12.6. Relaciones

- duplicado exacto;
- mismo nombre distinto contenido;
- copia entre contextos;
- versión probable;
- adjunto;
- hilo;
- árbol injertado;
- similitud.

## 12.7. Revisión

- decisiones ambiguas;
- riesgo alto;
- contexto protegido;
- colisiones;
- IA;
- plugins.

## 12.8. Plan

- cobertura completa;
- destinos;
- razones;
- riesgos;
- aprobaciones;
- hash del plan.

## 12.9. Ejecución

- copia parcial;
- flush;
- hash;
- rename atómico;
- registro;
- reanudación.

## 12.10. Verificación

- hashes;
- cobertura;
- destinos;
- parciales;
- archivos no registrados;
- modificaciones del origen;
- métricas.

---

# 13. Algoritmo de escaneo

## 13.1. Objetivos

- millones de entradas;
- memoria acotada;
- cancelación;
- reanudación;
- compatibilidad Unicode;
- Windows long paths;
- errores parciales;
- no seguir enlaces por defecto.

## 13.2. Recorrido

Usar cola iterativa.

```rust
while let Some(directory) = queue.pop_front() {
    for entry in read_directory(directory) {
        match classify_entry(entry) {
            Directory => queue.push_back(entry),
            File => persist_occurrence(entry),
            ReparsePoint => persist_without_following(entry),
            Error => persist_error(entry),
        }
    }
    commit_batch_if_needed();
    emit_progress();
}
```

## 13.3. Batch

Configurable:

```text
batch_entries = 1.000–10.000
batch_time = 250–1.000 ms
```

Elegir mediante benchmark.

## 13.4. Normalización

Guardar:

- ruta raw;
- ruta display;
- ruta comparativa.

No convertir destructivamente.

## 13.5. Identidad Windows

Cuando sea viable:

```text
volume serial
file index
size
mtime
```

## 13.6. Reparse points

Estados:

```text
SEEN_NOT_FOLLOWED
FOLLOWED_BY_EXPLICIT_POLICY
BROKEN
LOOP_DETECTED
```

---

# 14. Identidad y hashing

## 14.1. Fingerprint físico

Evita rehacer trabajo cuando el archivo no cambia.

## 14.2. BLAKE3

Usos:

- caché;
- firma rápida;
- chunks;
- árboles;
- índices internos.

## 14.3. SHA-256

Usos:

- manifiesto;
- verificación;
- exportación;
- interoperabilidad;
- auditoría.

## 14.4. Estrategia de dos pasos

### Modo rápido

1. agrupar por tamaño;
2. BLAKE3 de candidatos;
3. SHA-256 de relaciones relevantes.

### Modo completo

1. BLAKE3 de todo;
2. SHA-256 de todo.

El perfil jurídico recomienda modo completo.

## 14.5. Invalidación

Antes y después:

```text
fingerprint_before == fingerprint_after
```

Si cambia:

```text
SOURCE_CHANGED_DURING_HASH
```

---

# 15. Duplicados exactos

## 15.1. Definición

Dos contenidos son exactos cuando:

```text
size igual
SHA-256 igual
```

## 15.2. No inferir prescindibilidad

Considerar:

- contexto;
- perfil;
- ubicación;
- permisos;
- expediente;
- pericial;
- origen genérico;
- copia de seguridad;
- estado de revisión.

## 15.3. Tipos de duplicado

```text
WITHIN_SAME_CONTEXT
ACROSS_PROTECTED_CONTEXTS
GENERIC_TO_CANONICAL
ACTIVE_TO_EXCLUDED
BACKUP_REPLICA
UNKNOWN_CONTEXT
```

## 15.4. Políticas

```text
REPORT_ONLY
CONSOLIDATE_WITHIN_CONTEXT
CONSOLIDATE_GENERIC_COPIES
CONSOLIDATE_ALL
PRESERVE_ALL
```

## 15.5. Representante lógico

Puntuación configurable:

```text
+ contexto específico
+ nombre limpio
+ ruta canónica
+ fecha coherente
+ menor anomalía
- Descargas
- Escritorio
- Backup
- Copia
- ruta injertada
- temporal
```

El representante lógico no implica borrar otras apariciones.

---

# 16. Identidad parcial y FastCDC

## 16.1. Finalidad

Detectar:

- versiones;
- contenido parcialmente compartido;
- archivos truncados;
- documentos recompuestos;
- backups incrementales;
- contenedores relacionados.

## 16.2. Parámetros iniciales

No fijar definitivamente sin benchmark.

Configuración candidata:

```text
min chunk: 16 KiB
avg chunk: 64 KiB
max chunk: 256 KiB
```

Crear perfiles para:

- documentos;
- multimedia;
- archivos grandes;
- contenedores.

## 16.3. Similitud

```text
shared_bytes / union_bytes
```

Guardar:

- porcentaje;
- chunks compartidos;
- dirección temporal;
- evidencia.

## 16.4. No usar en 0.1

Implementar desde 0.3.

---

# 17. Similitud textual

## 17.1. Pipeline

```text
extract
normalize
segment
tokenize
shingle
MinHash
LSH candidates
detailed comparison
```

## 17.2. Normalización conservadora

Puede normalizar:

- Unicode;
- espacios;
- saltos;
- cabeceras técnicas conocidas.

No eliminar:

- importes;
- fechas;
- cláusulas;
- nombres;
- identificadores;
- numeración.

## 17.3. Relaciones

```text
TEXTUAL_DUPLICATE
FORMAT_VARIANT
LIKELY_VERSION
DERIVED_DOCUMENT
RELATED_DOCUMENT
```

## 17.4. Precaución

La similitud nunca autoriza eliminación automática.

---

# 18. Grafo contextual

## 18.1. Objetivo

Reconstruir:

- asuntos;
- clientes;
- personas;
- proyectos;
- expedientes;
- periciales;
- recursos;
- correspondencia;
- material recuperado.

## 18.2. Anclas fuertes

- número de procedimiento;
- identificador fiscal;
- nombre completo;
- dirección de email;
- carpeta canónica;
- Message-ID;
- asunto inequívoco;
- código de proyecto;
- número de expediente.

## 18.3. Señales ponderadas

Ejemplo inicial:

```text
100 coincidencia de ID único
95 carpeta canónica
90 Message-ID/hilo
85 email conocido
80 nombre completo único
75 número de procedimiento
70 entidad en nombre
60 entidad en contenido
50 adjunto de correo
40 proximidad temporal
30 nombre parcial
20 similitud semántica
```

Penalizaciones:

```text
-50 Descargas
-45 Escritorio
-40 Backup
-35 Recuperado
-30 carpeta genérica
-25 múltiples destinos
-20 confianza de extracción baja
```

## 18.4. Propagación

Aplicar propagación acotada:

- máximo de saltos;
- pérdida de peso;
- límites protegidos;
- evidencia acumulada.

## 18.5. Persistencia

Guardar todas las relaciones y su evidencia.

---

# 19. Árboles injertados y firmas de carpeta

## 19.1. Problema

Carpetas completas pueden aparecer:

- dentro de sí mismas;
- dentro de otra materia;
- copiadas desde backups;
- renombradas;
- parcialmente mezcladas.

## 19.2. Firma Merkle

```text
folder_hash =
hash(
  sorted(
    child_name_normalized
    + child_kind
    + child_content_hash_or_folder_hash
  )
)
```

## 19.3. Relaciones

```text
EXACT_TREE_CLONE
PARTIAL_TREE_CLONE
TREE_EMBEDDED
REPEATED_COMPONENT_ONLY
UNIQUE_CONTENT_IN_CLONE
```

## 19.4. Regla de seguridad

No eliminar una rama completa hasta identificar contenido único.

## 19.5. Resultado

Generar plan mixto:

- representados;
- preservados;
- únicos;
- ambiguos.

---

# 20. Correo

## 20.1. Formatos

### Inicial

- EML.

### Posterior

- MSG;
- MBOX;
- PST;
- OST.

## 20.2. Metadatos

```text
Message-ID
In-Reply-To
References
From
To
Cc
Date
Subject
Body hash
Attachment hashes
```

## 20.3. Hilos

Prioridad:

1. Message-ID.
2. References.
3. In-Reply-To.
4. asunto normalizado + participantes + fecha.
5. cuerpo citado.

## 20.4. Asociación a contexto

Combinar:

- carpeta;
- participantes;
- asunto;
- adjuntos;
- entidades;
- fechas.

## 20.5. Ambigüedad

No asignar si hay varios candidatos cercanos.

---

# 21. Archivos comprimidos y contenedores

## 21.1. Modelo virtual

```text
archive.zip
└── virtual entries
```

Cada entrada:

```text
container_id
virtual_path
size
crc
content hash opcional
```

## 21.2. Seguridad

- límites de expansión;
- zip bomb detection;
- profundidad;
- número de entradas;
- ratio;
- path traversal;
- nombres ilegales.

## 21.3. Política

No extraer físicamente por defecto.

---

# 22. Imagen

## 22.1. Niveles

1. SHA-256.
2. hash de píxeles normalizados.
3. perceptual hash.
4. embeddings opcionales posteriores.

## 22.2. Transformaciones

Detectar potencialmente:

- recompression;
- resize;
- rotation;
- EXIF changes;
- crop.

## 22.3. Periciales

Toda relación visual es informativa.

No autoriza consolidación.

---

# 23. Audio

## 23.1. Niveles

1. SHA-256.
2. metadatos.
3. duración.
4. Chromaprint.
5. forma de onda opcional.

## 23.2. Casos

- misma grabación en formatos distintos;
- audio recortado;
- duplicado musical;
- grabación jurídica;
- archivo con nombre engañoso.

---

# 24. Vídeo

## 24.1. Pipeline

```text
metadata
duration
keyframes
perceptual hashes
audio fingerprint
timeline matching
```

## 24.2. Fase

No implementar antes de 0.5.

---

# 25. Reglas y perfiles

## 25.1. Rule engine

Reglas declarativas en archivos versionados.

Ejemplo:

```yaml
id: temporary.office-lock
version: 1
match:
  file_name_glob: "~$*"
classification:
  category: temporary
  confidence: 1.0
action:
  default: COPY_TEMPORARY
risk: low
```

## 25.2. Perfil

Un perfil define:

- contextos;
- categorías;
- fronteras;
- pesos;
- reglas;
- buckets;
- política de duplicados;
- umbrales;
- revisión.

## 25.3. Perfil jurídico

Protege:

- expedientes;
- periciales;
- personas;
- asuntos;
- correspondencia.

## 25.4. Perfil genérico

Más conservador.

No intenta inferir sectores.

---

# 26. Planificación

## 26.1. Tipos de operación

```text
COPY_ACTIVE
COPY_REVIEW
COPY_SEPARATED
COPY_TEMPORARY
COPY_WITH_SUFFIX
SKIP_REPRESENTED
PRESERVE_ACROSS_CONTEXT
CREATE_DIRECTORY
NO_ACTION
BLOCKED
```

## 26.2. Cobertura

Cada aparición debe estar representada en el plan.

## 26.3. Idempotencia

```text
hash(
 project
 snapshot
 plan version
 occurrence
 operation
 destination
)
```

## 26.4. Plan inmutable

Al aprobar:

- serializar canónicamente;
- calcular SHA-256;
- bloquear cambios;
- registrar actor y fecha.

## 26.5. Validación

- ningún destino fuera de raíz;
- ninguna sobrescritura;
- ninguna colisión sin resolver;
- espacio;
- permisos;
- cobertura;
- operaciones bloqueadas;
- rutas;
- dependencias.

---

# 27. Ejecución segura

## 27.1. Protocolo por archivo

```text
validate source fingerprint
reserve destination
create partial
stream copy
flush
hash partial
compare
atomic finalize
record result
emit event
```

## 27.2. Parcial

```text
.<name>.dataforge-partial-<operation-id>
```

## 27.3. Colisiones

Si destino existe:

- mismo hash: `SKIP_REPRESENTED`;
- distinto hash: sufijo determinista;
- no sobrescribir.

## 27.4. Reanudación

Estados:

```text
PENDING
RUNNING
COPIED_PARTIAL
HASH_VERIFIED
COMPLETED
FAILED_RETRYABLE
FAILED_FINAL
BLOCKED
```

## 27.5. Errores

```text
SOURCE_CHANGED
SOURCE_MISSING
PERMISSION_DENIED
NO_SPACE
HASH_MISMATCH
DESTINATION_CHANGED
INVALID_PATH
IO_ERROR
PLUGIN_ERROR
```

---

# 28. Verificación

## 28.1. Cobertura

```text
source occurrences =
copied
+ represented
+ preserved
+ review
+ separated
+ blocked
+ unreadable
+ explicit no-action
```

## 28.2. Invariantes

- hash de copia válido;
- no sobrescritura;
- no modificación de origen;
- no parcial;
- no destino no registrado;
- no operación sin estado;
- no ruta fuera;
- métricas consistentes.

## 28.3. Ámbitos

### Operational scope

Contenido de uso diario.

### Physical container scope

Todo el contenedor:

- operativo;
- revisión;
- separado;
- temporales;
- informes.

### Audit scope

Metadatos e informes fuera de la salida documental.

---

# 29. Ledger de auditoría

## 29.1. Evento

```rust
pub struct AuditEvent {
    pub id: EventId,
    pub project_id: ProjectId,
    pub sequence: u64,
    pub timestamp: Timestamp,
    pub previous_hash: Digest,
    pub event_type: String,
    pub payload_hash: Digest,
    pub actor: Actor,
    pub event_hash: Digest,
}
```

## 29.2. Encadenamiento

```text
event_hash =
SHA-256(previous_hash + canonical_payload)
```

## 29.3. Merkle root

El manifiesto final puede producir una raíz Merkle.

## 29.4. Firma

Ed25519 opcional en fase posterior.

---

# 30. Búsqueda y analítica

## 30.1. SQLite

Transacciones y estado.

## 30.2. Parquet

Snapshots analíticos.

## 30.3. DataFusion

Consultas:

- extensiones;
- tamaños;
- contextos;
- grupos;
- riesgos;
- errores;
- tendencias.

## 30.4. Tantivy

Campos:

```text
file_name
relative_path
text
subject
from
to
entities
context
mime
```

## 30.5. No introducir antes de 0.4

---

# 31. Sistema de plugins

## 31.1. Tipos

```text
extractor
classifier
rule_pack
similarity
reporter
connector
profile
```

## 31.2. Modelo WIT inicial

```wit
package dataforge:plugin@0.1.0;

interface host {
    read-metadata: func(subject-id: string) -> string;
    read-range: func(subject-id: string, offset: u64, length: u32) -> list<u8>;
    emit-finding: func(payload-json: string);
    log: func(level: string, message: string);
}

world dataforge-plugin {
    import host;
    export describe: func() -> string;
    export analyze: func(subject-id: string, config-json: string) -> string;
}
```

## 31.3. Capacidades

Por defecto:

- sin red;
- sin reloj preciso salvo necesidad;
- sin filesystem;
- sin shell;
- memoria limitada;
- tiempo limitado;
- bytes limitados.

## 31.4. Registro

Cada plugin declara:

- ID;
- versión;
- licencia;
- autor;
- capacidades;
- schemas;
- compatibilidad;
- hash.

## 31.5. Fase

0.6.

---

# 32. Inteligencia artificial

## 32.1. Papel

- clasificación ambigua;
- explicación;
- resumen de contexto;
- sugerencia de etiquetas;
- redacción de informe.

## 32.2. Prohibiciones

- shell;
- borrado;
- movimiento;
- acceso directo;
- cambios de plan;
- aprobación;
- ejecución.

## 32.3. Proveedores

- local OpenAI-compatible;
- LM Studio;
- Ollama;
- cloud opt-in.

## 32.4. Contrato

JSON Schema.

## 32.5. Privacidad

Mostrar exactamente:

- qué campos;
- qué fragmentos;
- qué proveedor;
- qué endpoint.

## 32.6. Fase

Después del núcleo y antes de 1.0 solo como experimental.

---

# 33. CLI

## Comandos 0.1

```bash
dataforge init
dataforge project create
dataforge project open
dataforge project status
dataforge scan
dataforge hash
dataforge analyze
dataforge plan create
dataforge plan validate
dataforge plan approve
dataforge execute
dataforge verify
dataforge report
dataforge audit verify
```

## Salida

```text
--json
--jsonl
--quiet
--verbose
--no-color
```

## Código de salida

Documentar:

```text
0 success
1 generic failure
2 validation failure
3 partial completion
4 verification failure
5 permission failure
6 insufficient space
```

---

# 34. Aplicación de escritorio

## 34.1. Principios UX

- progresiva;
- lenguaje claro;
- modo simple;
- modo avanzado;
- vista técnica desplegable;
- decisiones agrupadas;
- sin terminal;
- cancelación segura;
- estado visible.

## 34.2. Pantallas

1. Inicio.
2. Nuevo proyecto.
3. Validación.
4. Escaneo.
5. Diagnóstico.
6. Relaciones.
7. Revisión.
8. Plan.
9. Ejecución.
10. Verificación.
11. Resultado.
12. Informes.
13. Configuración.
14. Plugins.
15. Acerca de.

## 34.3. Accesibilidad

- navegación teclado;
- contraste;
- escalado;
- lectores;
- mensajes no dependientes del color;
- progreso textual.

---

# 35. Estructura de proyecto en disco

```text
MyDataForgeProject/
├── project.dataforge.json
├── state/
│   └── dataforge.sqlite
├── snapshots/
│   ├── snapshot-0001.parquet
│   └── snapshot-0001.json
├── plans/
│   ├── plan-0001.json
│   └── plan-0001.csv
├── execution/
│   └── execution-0001.jsonl
├── indexes/
│   └── tantivy/
├── reports/
├── exports/
├── logs/
└── plugins/
```

El proyecto no contiene copias documentales salvo que el usuario seleccione el proyecto como salida, lo cual debe impedirse.

---

# 36. Formatos versionados

## 36.1. Encabezado común

```json
{
  "schema": "dataforge.plan",
  "schema_version": "1.0.0",
  "project_id": "...",
  "snapshot_id": "...",
  "created_at": "...",
  "generator_version": "..."
}
```

## 36.2. Compatibilidad

- semantic versioning;
- migraciones;
- reader backward-compatible cuando sea viable;
- errores explícitos.

---

# 37. Seguridad

## 37.1. Modelo de amenazas inicial

Amenazas:

- borrar origen;
- sobrescribir destino;
- path traversal;
- symlink escape;
- reparse loop;
- zip bomb;
- plugin malicioso;
- documento malformado;
- agotamiento de disco;
- agotamiento de memoria;
- SQL corruption;
- logs con datos sensibles;
- API key expuesta;
- actualización comprometida;
- informes manipulados.

## 37.2. Mitigaciones

- origen inmutable;
- canonicalización validada;
- límites;
- sandbox;
- hash;
- transacciones;
- ledger;
- backups de DB;
- logs redactables;
- secretos del sistema;
- releases firmadas;
- dependencias auditadas.

## 37.3. Herramientas CI

- `cargo audit`;
- `cargo deny`;
- Clippy;
- rustfmt;
- TypeScript;
- tests;
- fuzzing;
- CodeQL si procede;
- dependency review.

## 37.4. SECURITY.md

Incluir proceso privado de reporte.

---

# 38. Privacidad

## 38.1. Telemetría

Ninguna en 0.x.

## 38.2. Crash reports

Opt-in futuro y redactados.

## 38.3. Logs

Niveles de privacidad:

```text
FULL
REDACT_PATHS
HASH_ONLY
SUPPORT_BUNDLE
```

## 38.4. IA cloud

Desactivada por defecto.

---

# 39. Pruebas

## 39.1. Unitarias

- rutas;
- hashes;
- reglas;
- contexto;
- plan;
- estado;
- ledger;
- colisiones;
- serializers.

## 39.2. Integración

- proyecto completo;
- interrupción;
- reanudación;
- hash mismatch;
- disco lleno;
- permisos;
- red;
- Unicode.

## 39.3. Property-based

- nunca sobrescribe;
- nunca escapa salida;
- mismo hash agrupa;
- distinto hash no duplica;
- plan aprobado inmutable;
- completed implica hash válido.

## 39.4. Fuzzing

- rutas;
- nombres;
- JSON;
- plan;
- archivo comprimido;
- plugin output;
- extractores.

## 39.5. E2E

- app;
- CLI;
- portable;
- equipo limpio.

---

# 40. Corpus de regresión

## 40.1. Caso jurídico sintético

Incluir:

- expedientes;
- correos;
- periciales;
- recursos;
- temporales;
- media;
- descargas;
- árbol injertado;
- colisiones;
- duplicados.

## 40.2. Decisiones heredadas

### Periciales

Mismo nombre, distinto hash: preservar.

### Agentes

Ruta repetida con copia limpia: preferir canónica.

### Repetición legítima

No colapsar por patrón aislado.

### Correo

Asignar solo con evidencia suficiente.

### Media

Contexto prevalece sobre extensión.

### Ámbitos

Operativo y físico se informan por separado.

## 40.3. Datos reales

Nunca incluir datos reales sin anonimización verificable.

---

# 41. Benchmarks

## 41.1. Escaneo

- 10.000;
- 100.000;
- 1.000.000 entradas.

## 41.2. Hash

- HDD;
- SSD;
- NVMe;
- NAS;
- archivos pequeños;
- archivos grandes.

## 41.3. Memoria

Objetivo:

- streaming;
- memoria no proporcional al tamaño total;
- colas acotadas.

## 41.4. Publicación

No prometer cifras hasta medirlas.

---

# 42. Observabilidad

## 42.1. Eventos

```text
timestamp
project_id
task_id
component
event
severity
subject_id
operation_id
payload
```

## 42.2. Progreso

- archivos;
- bytes;
- velocidad;
- ETA solo si es estable;
- errores;
- fase.

## 42.3. Fuente de estado

SQLite, no logs.

---

# 43. Open source

## 43.1. Licencia

Propuesta:

```text
MIT OR Apache-2.0
```

Evaluar definitivamente antes de publicar.

## 43.2. Gobernanza inicial

### Benevolent maintainer

Luis Cordero mantiene dirección de producto.

### Maintainers

Acceso según:

- contribución;
- revisión;
- conducta;
- conocimiento.

### RFC

Cambios grandes requieren RFC.

### ADR

Decisiones técnicas requieren ADR.

## 43.3. DCO

Usar Developer Certificate of Origin para contribuciones.

No introducir CLA inicialmente.

## 43.4. Código de conducta

Contributor Covenant.

## 43.5. Releases

- changelog;
- checksums;
- SBOM;
- firma;
- notas;
- reproducibilidad progresiva.

---

# 44. Modelo comercial compatible

El motor seguirá abierto.

Posibles servicios:

- builds firmadas;
- soporte;
- implantación;
- perfiles profesionales;
- auditoría;
- conectores;
- formación;
- marca blanca;
- administración central;
- políticas empresariales;
- revisión humana;
- informes personalizados.

No degradar deliberadamente el motor abierto.

---

# 45. Roadmap maestro

El roadmap queda fijado por capacidades, no por fechas.

No avanzar a un hito sin cumplir criterios del anterior.

---

## Milestone 0.0 — Repository Foundation

### Objetivo

Repositorio profesional, compilable y gobernable.

### Entregables

- inspección automática del entorno;
- instalación autónoma de herramientas necesarias;
- plugins y skills de desarrollo documentados;
- scripts de bootstrap idempotentes;
- informe de entorno;
- monorepo;
- workspace Cargo;
- Tauri;
- React;
- TypeScript strict;
- CLI mínima;
- CI;
- licencias;
- README;
- CONTRIBUTING;
- SECURITY;
- GOVERNANCE;
- Code of Conduct;
- ADR template;
- RFC template;
- issue templates;
- release profile.

### Criterios de aceptación

- bootstrap reproducible en Windows;
- herramientas verificadas;
- plugins y skills documentados;
- informe de entorno generado;
- build Windows;
- tests;
- lint;
- app abre;
- CLI responde;
- CI verde;
- no funcionalidad falsa.

### Salida

`v0.0.1-dev`

---

## Milestone 0.1 — Safe Inventory Core

### Objetivo

Inventariar y copiar de forma segura.

### Capacidades

- proyecto;
- validación;
- SQLite;
- máquina de estados;
- escaneo;
- fingerprints;
- BLAKE3;
- SHA-256;
- duplicados exactos;
- plan;
- copia;
- verificación;
- ledger;
- CLI;
- interfaz básica;
- informes mínimos.

### Exclusiones

- chunks;
- texto;
- correo avanzado;
- búsqueda;
- plugins;
- IA.

### Criterios

1. 100.000 archivos.
2. origen sin cambios.
3. copia verificada.
4. reanudación.
5. no sobrescritura.
6. cobertura.
7. plan inmutable.
8. ledger válido.
9. portable.
10. corpus base.

### Release

`v0.1.0`

---

## Milestone 0.2 — Structural Intelligence

### Objetivo

Comprender estructura y contexto.

### Capacidades

- grafo de carpetas;
- contextos;
- fronteras;
- perfiles;
- reglas declarativas;
- anomalías;
- árboles Merkle;
- duplicados por contexto;
- revisión;
- UI de diagnóstico.

### Criterios

- detectar árboles injertados;
- preservar contenido único;
- diferenciar repetición legítima;
- políticas de duplicado;
- perfil jurídico sintético;
- evidencia por decisión.

### Release

`v0.2.0`

---

## Milestone 0.3 — Similarity and Versioning

### Objetivo

Detectar relación parcial y versiones.

### Capacidades

- FastCDC;
- chunks;
- BLAKE3 de chunks;
- similitud;
- MinHash;
- LSH;
- linaje;
- candidatos;
- visualización de versiones.

### Criterios

- detectar variantes sintéticas;
- no confundir similitud con identidad;
- benchmarks;
- almacenamiento acotado;
- umbrales configurables.

### Release

`v0.3.0`

---

## Milestone 0.4 — Content Intelligence

### Objetivo

Comprender contenido documental.

### Capacidades

- extractores;
- PDF;
- DOCX;
- TXT;
- HTML;
- EML;
- ZIP;
- texto normalizado;
- Tantivy;
- Parquet;
- DataFusion;
- búsqueda;
- consultas.

### Criterios

- búsqueda con ruta y contexto;
- hilos EML básicos;
- adjuntos;
- zip seguro;
- SQL analítico;
- índice reconstruible.

### Release

`v0.4.0`

---

## Milestone 0.5 — Media Intelligence

### Objetivo

Relacionar imágenes, audio y vídeo.

### Capacidades

- image hashes;
- pHash;
- Chromaprint;
- vídeo por keyframes;
- fingerprints;
- contexto;
- revisión multimedia.

### Criterios

- recompression;
- resize;
- audio transcodificado;
- vídeo recomprimido;
- ninguna eliminación automática.

### Release

`v0.5.0`

---

## Milestone 0.6 — Plugin Ecosystem

### Objetivo

Extensión segura.

### Capacidades

- Wasmtime;
- WASI;
- WIT;
- SDK;
- registro;
- permisos;
- límites;
- plugins de ejemplo.

### Criterios

- plugin sin filesystem;
- timeout;
- memoria;
- firma/hash;
- compatibilidad;
- documentación.

### Release

`v0.6.0`

---

## Milestone 0.7 — Assisted Intelligence

### Objetivo

IA opcional y controlada.

### Capacidades

- proveedor abstracto;
- local;
- cloud opt-in;
- JSON Schema;
- prompts;
- redacción;
- auditoría;
- explicaciones;
- sugerencias.

### Criterios

- funciona sin IA;
- IA no ejecuta;
- datos visibles;
- respuesta validada;
- riesgo recalculado;
- tests contra prompt injection documental.

### Release

`v0.7.0`

---

## Milestone 0.8 — Cross-platform and Scale

### Objetivo

Portabilidad y grandes volúmenes.

### Capacidades

- macOS experimental;
- Linux experimental;
- 1M+ archivos;
- mejoras NAS;
- snapshots incrementales;
- cache;
- daemon experimental.

### Release

`v0.8.0`

---

## Milestone 0.9 — Stabilization

### Objetivo

Congelar contratos hacia 1.0.

### Capacidades

- schemas;
- API;
- plugin ABI;
- migraciones;
- documentación;
- threat model;
- reproducible builds;
- SBOM;
- firmas;
- UX final;
- accesibilidad.

### Criterios

- sin defectos críticos conocidos;
- corpus completo;
- fuzzing;
- migraciones;
- compatibilidad;
- documentación de usuario.

### Release

`v0.9.0`

---

## Milestone 1.0 — Stable Reconstruction Platform

### Objetivo

Primera versión estable.

### Garantías

- origen inmutable;
- planes versionados;
- copia verificada;
- reanudación;
- formatos estables;
- plugins compatibles;
- búsqueda;
- perfiles;
- informes;
- seguridad documentada;
- builds firmadas;
- soporte de migración.

### Release

`v1.0.0`

---

# 46. Backlog inicial ordenado

## Epic A — Repository

- [ ] Crear repo.
- [ ] Licencias.
- [ ] Rust workspace.
- [ ] pnpm workspace.
- [ ] Tauri app.
- [ ] CLI.
- [ ] CI.
- [ ] Templates.
- [ ] Governance.
- [ ] Security.

## Epic B — Domain

- [ ] IDs tipados.
- [ ] errores.
- [ ] estados.
- [ ] eventos.
- [ ] entidades.
- [ ] serialization.
- [ ] tests.

## Epic C — Database

- [ ] SQLite.
- [ ] migraciones.
- [ ] repositories.
- [ ] transaction manager.
- [ ] WAL benchmark.
- [ ] backup.
- [ ] integrity check.

## Epic D — Scan

- [ ] source validation.
- [ ] directory walker.
- [ ] batching.
- [ ] progress.
- [ ] pause.
- [ ] cancel.
- [ ] reparse.
- [ ] Unicode.
- [ ] long paths.
- [ ] fixtures.

## Epic E — Hash

- [ ] BLAKE3.
- [ ] SHA-256.
- [ ] fingerprint.
- [ ] invalidation.
- [ ] cache.
- [ ] progress.
- [ ] duplicates.

## Epic F — Plan

- [ ] operation model.
- [ ] destination builder.
- [ ] collision policy.
- [ ] coverage.
- [ ] validation.
- [ ] versioning.
- [ ] approval.
- [ ] hash.

## Epic G — Execute

- [ ] partial.
- [ ] streaming.
- [ ] flush.
- [ ] verify.
- [ ] atomic finalize.
- [ ] resume.
- [ ] errors.
- [ ] disk full.

## Epic H — Verify

- [ ] manifest.
- [ ] coverage.
- [ ] hashes.
- [ ] partials.
- [ ] untracked.
- [ ] origin checks.
- [ ] metrics.

## Epic I — Ledger

- [ ] canonical JSON.
- [ ] chain.
- [ ] verify.
- [ ] export.
- [ ] tamper test.

## Epic J — Desktop

- [ ] project wizard.
- [ ] validation.
- [ ] progress.
- [ ] diagnostic.
- [ ] plan review.
- [ ] execution.
- [ ] report.

---

# 47. Primer sprint para Codex

## Objetivo

Crear la fundación, no una demo falsa.

## Tareas

### 1. Bootstrap

- workspace Rust;
- Tauri 2;
- React;
- TypeScript strict;
- pnpm;
- CLI;
- app.

### 2. Documentación

Crear:

```text
docs/adr/ADR-0001-rust-core.md
docs/adr/ADR-0002-sqlite-source-of-truth.md
docs/adr/ADR-0003-origin-immutable.md
docs/architecture/system-overview.md
docs/threat-model/initial.md
```

### 3. Dominio

Implementar:

- typed IDs;
- Project;
- ProjectState;
- SourceRoot;
- Snapshot;
- AuditEvent.

### 4. DB

- migration 0001;
- create project;
- state transition;
- event append;
- integrity test.

### 5. CLI

```bash
dataforge project create
dataforge project status
```

### 6. Desktop

- pantalla inicio;
- crear proyecto;
- abrir proyecto;
- mostrar estado.

### 7. Tests

- state transitions;
- invalid transition;
- event chain;
- serialization;
- database transaction.

## No hacer

- scanner real;
- hash;
- IA;
- plugins;
- mock de resultados;
- botones sin estado real.

## Criterio de cierre

- app y CLI usan el mismo `df-facade`;
- DB real;
- tests verdes;
- CI;
- documentación;
- ningún estado simulado.

---

# 48. Prompt maestro para Codex

```text
Estás trabajando en DataForge, un motor open source local-first de reconstrucción documental.

Lee RFC-0001-DATAFORGE-FOUNDATION-AND-ROADMAP.md completo antes de modificar archivos.

Antes de implementar:
- inspecciona el entorno;
- detecta herramientas, plugins y skills ya disponibles;
- instala autónomamente los que falten y sean necesarios;
- usa fuentes oficiales o verificables;
- comprueba versiones y compatibilidad;
- crea scripts de bootstrap idempotentes;
- documenta instalaciones, plugins, skills, comandos, fuentes e incidencias;
- crea skills propias del repositorio para procesos repetibles;
- continúa sin pedir confirmación por dependencias que puedas resolver de forma segura;
- no uses privilegios, credenciales o cambios globales salvo que sean imprescindibles;
- si una instalación queda bloqueada, continúa con todas las tareas independientes.

Reglas:
- El origen es inmutable.
- No implementes borrado.
- No implementes sobrescritura.
- SQLite es la única fuente de verdad.
- La interfaz no contiene lógica crítica.
- Todo cliente usa df-facade.
- No implementes IA, FastCDC, búsqueda ni plugins hasta su milestone.
- No añadas botones sin funcionalidad real.
- No declares una tarea terminada sin ejecutar pruebas.
- Mantén el repositorio compilable.
- Registra decisiones importantes en ADR.
- No cambies el roadmap sin RFC o ADR explícita.

Comienza únicamente por Milestone 0.0 y el Primer sprint.

Orden:
1. Inspeccionar el entorno.
2. Instalar y verificar autónomamente herramientas, plugins y skills necesarios.
3. Crear scripts de bootstrap y comprobación.
4. Generar `docs/environment-report.md`.
5. Crear monorepo.
6. Crear crates base.
7. Crear documentación.
8. Implementar dominio.
9. Implementar SQLite y migración inicial.
10. Implementar máquina de estados.
11. Implementar ledger mínimo.
12. Implementar CLI `project create/status`.
13. Implementar UI `create/open/status` usando `df-facade`.
14. Ejecutar fmt, clippy, tests y build.

Al terminar:
- presenta archivos creados;
- comandos ejecutados;
- pruebas;
- riesgos;
- tareas pendientes;
- no avances al escáner sin que Milestone 0.0 esté validado.
```

---

# 49. Definition of Done general

Una tarea está terminada cuando:

- código implementado;
- tests;
- errores tipados;
- documentación;
- lint;
- build;
- no regresión;
- criterios cumplidos;
- sin TODO oculto;
- estado real reflejado en README.

---

# 50. Criterios para modificar este roadmap

El roadmap puede evolucionar, pero no informalmente.

## Cambio menor

ADR:

- librería;
- implementación;
- organización;
- optimización.

## Cambio mayor

RFC:

- eliminar garantía;
- cambiar formato;
- cambiar lenguaje;
- introducir servicio externo;
- introducir borrado;
- cambiar licencia;
- cambiar sistema de plugins;
- modificar contrato estable.

---

# 51. Riesgos del proyecto

## Riesgo 1 — Alcance excesivo

Mitigación:

- milestones;
- capacidades cerradas;
- no anticipar tecnologías.

## Riesgo 2 — Optimización prematura

Mitigación:

- benchmark;
- métricas;
- implementación simple primero.

## Riesgo 3 — Confundir IA con producto

Mitigación:

- IA en 0.7;
- núcleo determinista.

## Riesgo 4 — Seguridad de archivos

Mitigación:

- invariantes;
- propiedad;
- fuzz;
- origen inmutable.

## Riesgo 5 — Formatos hostiles

Mitigación:

- plugins;
- sandbox;
- límites;
- parsers maduros.

## Riesgo 6 — Falta de comunidad

Mitigación:

- README claro;
- issues;
- good first issues;
- corpus;
- CLI útil;
- documentación.

## Riesgo 7 — Confianza comercial

Mitigación:

- open source;
- informes;
- ledger;
- builds firmadas;
- política clara.

---

# 52. Métricas de éxito

## Técnicas

- archivos procesados;
- bytes;
- throughput;
- memoria;
- errores;
- cobertura;
- reanudación;
- tiempos;
- falsos positivos.

## Producto

- proyectos completados;
- decisiones automáticas aceptadas;
- revisión necesaria;
- errores evitados;
- estructuras reconstruidas;
- satisfacción.

## Comunidad

- estrellas;
- issues resueltas;
- contribuyentes;
- plugins;
- perfiles;
- adopción.

No optimizar únicamente para métricas públicas.

---

# 53. Nombre y comunicación

## Nombre

DataForge.

## Descriptor

> Open-source document reconstruction engine.

En español:

> Motor open source de reconstrucción documental.

## Mensaje

> Ordena, reconstruye y migra archivos sin tocar el origen.

## Evitar

- “limpiador”;
- “borra duplicados”;
- “IA mágica”;
- “forense certificado” sin serlo;
- “garantía de recuperación”;
- “100 % automático”.

---

# 54. Conclusión fundacional

DataForge comienza como una herramienta local y segura, pero su arquitectura se diseña como plataforma.

El orden es deliberado:

```text
seguridad
→ identidad
→ estructura
→ similitud
→ contenido
→ multimedia
→ plugins
→ IA
→ estabilidad
```

No se construirá una interfaz llamativa encima de scripts inseguros.

Se construirá un motor abierto capaz de:

- explicar;
- verificar;
- recuperar;
- escalar;
- extenderse;
- ganarse la confianza del usuario.

Este documento fija el comienzo y el roadmap hasta DataForge 1.0.
