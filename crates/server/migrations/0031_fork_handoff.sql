-- A fork that runs on another machine than the one with its files waits for a
-- `handoff` run on that machine, which pushes the files for it; the handoff names
-- its fork in `context`.
CREATE INDEX runs_handoff_idx ON runs (context) WHERE kind = 'handoff';
