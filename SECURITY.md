# Política de seguridad

## Versiones soportadas

DataForge está desarrollando la serie `0.2.x`, todavía documentada como
`Unreleased`. No hay una release estable publicada; los informes de seguridad
se evalúan contra `main` y, cuando corresponda, contra la versión afectada.

## Cómo reportar una vulnerabilidad

**No abras un issue público.**

Envía un correo a **luiscor97@gmail.com** con asunto `[SECURITY] DataForge`
incluyendo:

- descripción y impacto;
- pasos de reproducción o PoC;
- versión/commit afectado;
- si procede, propuesta de mitigación.

Compromisos:

- acuse de recibo en 7 días;
- evaluación inicial y plan en 30 días;
- crédito en las notas de la corrección si lo deseas.

El repositorio es público: no incluyas detalles sensibles, adjuntos ni PoC en
issues, Discussions o pull requests. El correo anterior es el canal privado
responsable documentado actualmente.

## Ámbito

Nos interesa especialmente cualquier vía por la que el motor pudiera:

1. modificar, borrar o sobrescribir archivos de origen;
2. escribir fuera de la raíz de salida (path traversal, symlink escape);
3. falsificar o romper el ledger de auditoría sin detección;
4. ejecutar código a través de documentos o rutas hostiles.
