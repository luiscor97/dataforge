# ADR-0039 — Firma de release keyless (Sigstore) (M0.9)

- Estado: Aceptada (reversible sin coste hasta el primer tag)
- Fecha: 2026-07-21
- Contexto: RFC-0001 §M0.9/M1.0 (builds firmadas);
  `docs/release/m1.0-acceptance.md`

## Contexto

M1.0 exige builds firmadas. Las vías consideradas:

1. **Certificado OV/EV de code signing**: firma nativa de Windows
   (SmartScreen), pero cuesta dinero anual, exige verificación de identidad
   del propietario y custodiar una clave privada — infraestructura que el
   proyecto no tiene hoy.
2. **Sigstore keyless (cosign)**: sin claves que custodiar. El workflow de
   release intercambia su identidad OIDC de GitHub Actions por un
   certificado efímero; la firma queda ligada criptográficamente a *ese*
   repositorio, *ese* workflow y *ese* tag, y registrada en el log público
   de transparencia (Rekor).

## Decisión

Sigstore keyless para 1.0. El job de release firma `SHA256SUMS.txt` (cubre
todos los binarios transitivamente) y el SBOM con `cosign sign-blob`;
firma y certificado se adjuntan a la release en borrador junto a las
instrucciones de verificación.

El **acto humano** que autoriza la firma es empujar el tag `v*` — el mismo
acto que ya autoriza el build de release. Publicar la release (y con ella
las firmas) sigue siendo un click manual posterior sobre el borrador.

## Consecuencias

- Cualquiera puede verificar que un artefacto salió de este repositorio y
  este workflow, sin confiar en una clave que alguien pueda perder o filtrar.
- La entrada en el log de transparencia de Rekor es **pública y permanente**
  una vez firmado: otra razón por la que solo un tag deliberado dispara la
  firma.
- No hay firma Authenticode: SmartScreen tratará los `.exe` como no
  firmados. Si el proyecto quiere reputación SmartScreen más adelante, un
  certificado OV/EV puede **añadirse** sin retirar Sigstore (ambas firmas
  conviven); queda como decisión de producto post-1.0.
- Si el propietario prefiere la vía de certificado antes del primer tag,
  este ADR se sustituye sin deuda: el hueco del workflow es el mismo.
