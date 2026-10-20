// +build windows

package main

import "testing"

func TestIsReadOnlySQL(t *testing.T) {
	allowed := []string{
		"SELECT 1",
		"  select * from t",
		"-- comment\nSELECT 1",
		"/* comment */ WITH x AS (SELECT 1) SELECT * FROM x",
		"SHOW TABLES",
		"DESCRIBE t",
		"EXPLAIN SELECT 1",
		"PRAGMA table_info(t)",
	}
	for _, statement := range allowed {
		if !isReadOnlySQL(statement) {
			t.Fatalf("expected read-only: %q", statement)
		}
	}
	rejected := []string{
		"",
		"UPDATE t SET x=1",
		"DELETE FROM t",
		"INSERT INTO t VALUES (1)",
		"EXEC sp_who",
		"-- only a comment",
	}
	for _, statement := range rejected {
		if isReadOnlySQL(statement) {
			t.Fatalf("expected rejected: %q", statement)
		}
	}
}

func TestHasMultipleStatements(t *testing.T) {
	if hasMultipleStatements("SELECT 1;") {
		t.Fatal("trailing semicolon should be allowed")
	}
	if !hasMultipleStatements("SELECT 1; DELETE FROM t") {
		t.Fatal("expected multiple statements")
	}
	if hasMultipleStatements("SELECT ';' AS value") {
		t.Fatal("semicolon inside a string literal must not count")
	}
}
