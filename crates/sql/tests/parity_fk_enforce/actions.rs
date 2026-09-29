//! Referential actions: ON DELETE RESTRICT, SET NULL and SET DEFAULT, and
//! ON UPDATE CASCADE and SET NULL.

use super::*;

// -- ON DELETE RESTRICT ------------------------------------------------------

#[test]
fn delete_parent_restrict_errors() {
    assert_parity(
        "PRAGMA foreign_keys = ON;\
         CREATE TABLE parent(id INTEGER PRIMARY KEY);\
         CREATE TABLE child(\
             id INTEGER PRIMARY KEY,\
             pid INTEGER REFERENCES parent(id) ON DELETE RESTRICT\
         );\
         INSERT INTO parent VALUES (1);\
         INSERT INTO child VALUES (10, 1);\
         DELETE FROM parent WHERE id = 1;\
         SELECT id FROM parent ORDER BY id",
    );
}

// -- ON DELETE SET NULL ------------------------------------------------------

#[test]
fn delete_parent_set_null_nulls_children() {
    assert_parity(
        "PRAGMA foreign_keys = ON;\
         CREATE TABLE parent(id INTEGER PRIMARY KEY);\
         CREATE TABLE child(\
             id INTEGER PRIMARY KEY,\
             pid INTEGER REFERENCES parent(id) ON DELETE SET NULL\
         );\
         INSERT INTO parent VALUES (1);\
         INSERT INTO child VALUES (10, 1);\
         DELETE FROM parent WHERE id = 1;\
         SELECT id, pid FROM child ORDER BY id",
    );
}

// -- ON DELETE SET DEFAULT ---------------------------------------------------

#[test]
fn delete_parent_set_default_substitutes_default() {
    assert_parity(
        "PRAGMA foreign_keys = ON;\
         CREATE TABLE parent(id INTEGER PRIMARY KEY);\
         CREATE TABLE child(\
             id INTEGER PRIMARY KEY,\
             pid INTEGER DEFAULT 0 REFERENCES parent(id) ON DELETE SET DEFAULT\
         );\
         INSERT INTO parent VALUES (1);\
         INSERT INTO child VALUES (10, 1);\
         DELETE FROM parent WHERE id = 1;\
         SELECT id, pid FROM child ORDER BY id",
    );
}

// -- ON UPDATE CASCADE -------------------------------------------------------

#[test]
fn update_parent_cascade_propagates_to_children() {
    assert_parity(
        "PRAGMA foreign_keys = ON;\
         CREATE TABLE parent(id INTEGER PRIMARY KEY);\
         CREATE TABLE child(\
             id INTEGER PRIMARY KEY,\
             pid INTEGER REFERENCES parent(id) ON UPDATE CASCADE\
         );\
         INSERT INTO parent VALUES (1);\
         INSERT INTO child VALUES (10, 1);\
         UPDATE parent SET id = 7 WHERE id = 1;\
         SELECT id, pid FROM child ORDER BY id",
    );
}

#[test]
fn update_parent_set_null_nulls_children() {
    assert_parity(
        "PRAGMA foreign_keys = ON;\
         CREATE TABLE parent(id INTEGER PRIMARY KEY);\
         CREATE TABLE child(\
             id INTEGER PRIMARY KEY,\
             pid INTEGER REFERENCES parent(id) ON UPDATE SET NULL\
         );\
         INSERT INTO parent VALUES (1);\
         INSERT INTO child VALUES (10, 1);\
         UPDATE parent SET id = 99 WHERE id = 1;\
         SELECT id, pid FROM child ORDER BY id",
    );
}
