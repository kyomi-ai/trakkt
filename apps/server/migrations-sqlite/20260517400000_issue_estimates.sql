-- Add estimate column to issues for team-configurable estimation scales.
-- Estimate settings live in the teams.settings JSON column (no schema change needed).
ALTER TABLE issues ADD COLUMN estimate INTEGER;
