1. Objectif du projet

Développer un crate Rust 100 % safe Rust permettant de créer des archives 7z valides, compatibles avec l’outil officiel 7-Zip, en exploitant le multi-threading pour la compression.

Le projet est une librairie, pas un outil CLI.

2. Contraintes non négociables
Langage & sécurité

Rust stable uniquement

#![forbid(unsafe_code)]

Aucune dépendance FFI

Aucune dépendance C / C++

Dépendances autorisées

rayon (multi-threading)

thiserror ou équivalent (erreurs)

crc32fast

byteorder

Toute dépendance doit être justifiée

3. Scope fonctionnel (v0.1)
Fonctionnalités incluses

Création d’archives 7z

Compression LZMA2 uniquement

Compression multi-thread par blocs

Ajout de fichiers depuis le disque

Ajout de buffers mémoire

Génération d’un header 7z valide

Fonctionnalités explicitement exclues

Décompression

Chiffrement (AES)

Solid compression multi-fichiers

Filtres BCJ / Delta

Streaming en entrée

CLI

⚠️ Toute fonctionnalité non listée est hors scope.

4. Références obligatoires

Le développement doit se baser explicitement sur :

7z Format Specification (officielle)

Section Header

Section PackInfo / UnpackInfo

LZMA2 coder

Aucune “interprétation libre” du format n’est acceptée.

5. Architecture imposée
Organisation des modules
src/
 ├── lib.rs
 ├── archive/
 │    ├── builder.rs
 │    ├── writer.rs
 │    └── header.rs
 ├── compression/
 │    ├── lzma2.rs
 │    └── block.rs
 ├── threading/
 │    ├── scheduler.rs
 │    └── worker.rs
 ├── io/
 │    ├── writer.rs
 │    └── seek.rs
 └── error.rs

Principes

Séparation stricte des responsabilités

Aucun module ne doit connaître la totalité du format

Le threading ne doit jamais toucher au format binaire

6. API publique minimale

L’API publique doit exposer au minimum :

let mut archive = SevenZipWriter::new(output)?;

archive.add_file("local/path", "archive/path")?;
archive.add_bytes("data.bin", &data)?;

archive.finish()?;


Contraintes :

finish() est obligatoire

Aucune écriture implicite du header

Les erreurs doivent être explicites et typées

7. Stratégie de multi-threading
Règles

Découpage des fichiers en blocs indépendants

Chaque bloc est compressé en parallèle

Les métadonnées sont collectées puis assemblées en fin

Interdictions

Pas de spawn sauvage

Pas de global thread pool implicite

Pas de partage mutable non contrôlé

8. Écriture du fichier 7z

Ordre strict :

Écriture des blocs compressés

Collecte des offsets, tailles, CRC

Construction du header final

Écriture du header en fin de fichier

Le header ne doit jamais être écrit partiellement.

9. Gestion des erreurs

Une enum SevenZipError centralisée

Pas de unwrap

Pas de expect

Messages d’erreur exploitables par un développeur

10. Tests obligatoires
Tests unitaires

Sérialisation binaire des structures 7z

CRC

Compression LZMA2 par bloc

Tests d’intégration

Création d’une archive

Extraction avec 7-Zip officiel

Vérification par hash des fichiers extraits

Tout code non testable doit être justifié.

11. Critères de validation

Le projet est considéré valide si :

Une archive produite est extractible par 7-Zip

Les fichiers extraits sont strictement identiques

Aucun unsafe n’est présent

Le code compile sans warnings

Les tests passent

12. Interdits explicites

Copier du code depuis p7zip

Implémenter des fonctionnalités non demandées

Modifier le scope sans instruction explicite

Optimiser au détriment de la clarté ou de la validité du format

“Deviner” le format 7z

13. Livrables attendus

Code source structuré

Tests

README.md décrivant clairement :

ce que la librairie fait

ce qu’elle ne fait pas

14. Principe directeur

La validité du format et la clarté du code priment sur la performance. Les optimisations sont autorisées tant qu'elles ne compromettent ni la validité ni la lisibilité.
