-- A file you can feed the shell:
--   redlinedb /tmp/notes.redline < docs/manual/examples/first.sql
-- Run it once. A second run fails on CREATE TABLE, which is what you want
-- from a script that is showing the statements rather than migrating.

CREATE TABLE note (
  id INTEGER PRIMARY KEY,
  body TEXT NOT NULL
);

INSERT INTO note VALUES (1, 'hello');

SELECT id, body FROM note;
