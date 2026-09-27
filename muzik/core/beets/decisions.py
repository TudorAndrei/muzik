"""Compatibility names for import decisions."""

from muzik.core.import_models import (
    DuplicateDecision as BeetsDuplicateDecision,
    ImportDecisions as BeetsDecisions,
    MatchDecision as BeetsMatchDecision,
    NonInteractiveImportDecisions as NonInteractiveBeetsDecisions,
)

__all__ = [
    "BeetsDecisions",
    "BeetsDuplicateDecision",
    "BeetsMatchDecision",
    "NonInteractiveBeetsDecisions",
]
