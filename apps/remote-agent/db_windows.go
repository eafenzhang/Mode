// +build windows

package main

import (
	"encoding/base64"
	"runtime"
	"sort"
	"strings"
	"syscall"
	"unicode/utf8"
	"unsafe"
)

var odbc32 = syscall.NewLazyDLL("odbc32.dll")

var (
	procSQLAllocHandle     = odbc32.NewProc("SQLAllocHandle")
	procSQLFreeHandle      = odbc32.NewProc("SQLFreeHandle")
	procSQLSetEnvAttr      = odbc32.NewProc("SQLSetEnvAttr")
	procSQLSetConnectAttrW = odbc32.NewProc("SQLSetConnectAttrW")
	procSQLSetStmtAttrW    = odbc32.NewProc("SQLSetStmtAttrW")
	procSQLDriverConnectW  = odbc32.NewProc("SQLDriverConnectW")
	procSQLDisconnect      = odbc32.NewProc("SQLDisconnect")
	procSQLExecDirectW     = odbc32.NewProc("SQLExecDirectW")
	procSQLNumResultCols   = odbc32.NewProc("SQLNumResultCols")
	procSQLDescribeColW    = odbc32.NewProc("SQLDescribeColW")
	procSQLFetch           = odbc32.NewProc("SQLFetch")
	procSQLGetData         = odbc32.NewProc("SQLGetData")
	procSQLDriversW        = odbc32.NewProc("SQLDriversW")
	procSQLGetDiagRecW     = odbc32.NewProc("SQLGetDiagRecW")
)

const (
	sqlSuccess          = 0
	sqlSuccessWithInfo  = 1
	sqlNoData           = 100
	sqlHandleEnv        = 1
	sqlHandleDbc        = 2
	sqlHandleStmt       = 3
	sqlAttrODBCVersion  = 200
	sqlOVODBC3          = 3
	sqlNTS16            = uintptr(0xFFFD)
	sqlNTS32            = uintptr(0xFFFFFFFD)
	sqlDriverNoPrompt   = 0
	sqlCChar            = uintptr(1)
	sqlCBinary          = uintptr(0xFFFE) // SQL_C_BINARY (-2) as SQLSMALLINT
	sqlNullData         = -1
	sqlNoTotal          = -4
	sqlAttrQueryTimeout = 0
	sqlAttrMaxRows      = 1
	sqlAttrAccessMode   = 101
	sqlModeReadOnly     = 1
	sqlFetchFirst       = 2
	sqlFetchNext        = 1
	sqlMaxCellBytes     = 64 * 1024
)

func isSQLSuccess(code uintptr) bool {
	return int16(code) == sqlSuccess || int16(code) == sqlSuccessWithInfo
}

func odbcAllocateEnv() (uintptr, *APIError) {
	var env uintptr
	code, _, _ := procSQLAllocHandle.Call(sqlHandleEnv, 0, uintptr(unsafe.Pointer(&env)))
	if !isSQLSuccess(code) {
		return 0, apiError("db_unavailable", "ODBC environment allocation failed", 500)
	}
	code, _, _ = procSQLSetEnvAttr.Call(env, sqlAttrODBCVersion, sqlOVODBC3, 0)
	if !isSQLSuccess(code) {
		procSQLFreeHandle.Call(sqlHandleEnv, env)
		return 0, apiError("db_unavailable", "ODBC 3 environment setup failed", 500)
	}
	return env, nil
}

func odbcAllocateDbc(env uintptr) (uintptr, *APIError) {
	var dbc uintptr
	code, _, _ := procSQLAllocHandle.Call(sqlHandleDbc, env, uintptr(unsafe.Pointer(&dbc)))
	if !isSQLSuccess(code) {
		return 0, apiError("db_unavailable", "ODBC connection allocation failed", 500)
	}
	return dbc, nil
}

func odbcAllocateStmt(dbc uintptr) (uintptr, *APIError) {
	var stmt uintptr
	code, _, _ := procSQLAllocHandle.Call(sqlHandleStmt, dbc, uintptr(unsafe.Pointer(&stmt)))
	if !isSQLSuccess(code) {
		return 0, apiError("db_unavailable", "ODBC statement allocation failed", 500)
	}
	return stmt, nil
}

func odbcDiagnostic(handleType int16, handle uintptr) string {
	if handle == 0 {
		return ""
	}
	var state [6]uint16
	var nativeError int32
	var message [1024]uint16
	var textLength int16
	code, _, _ := procSQLGetDiagRecW.Call(
		uintptr(handleType),
		handle,
		1,
		uintptr(unsafe.Pointer(&state[0])),
		uintptr(unsafe.Pointer(&nativeError)),
		uintptr(unsafe.Pointer(&message[0])),
		uintptr(len(message)),
		uintptr(unsafe.Pointer(&textLength)),
	)
	if int16(code) != sqlSuccess && int16(code) != sqlSuccessWithInfo {
		return ""
	}
	return strings.TrimSpace(syscall.UTF16ToString(state[:]) + ": " + syscall.UTF16ToString(message[:]))
}

func odbcError(prefix string, env, dbc, stmt uintptr) *APIError {
	message := ""
	if stmt != 0 {
		message = odbcDiagnostic(sqlHandleStmt, stmt)
	}
	if message == "" && dbc != 0 {
		message = odbcDiagnostic(sqlHandleDbc, dbc)
	}
	if message == "" && env != 0 {
		message = odbcDiagnostic(sqlHandleEnv, env)
	}
	if message == "" {
		message = "unknown ODBC error"
	}
	return apiError("db_error", prefix+": "+message, 400)
}

func listDBDrivers() (*DBDriversResult, *APIError) {
	env, apiErr := odbcAllocateEnv()
	if apiErr != nil {
		return nil, apiErr
	}
	defer procSQLFreeHandle.Call(sqlHandleEnv, env)
	direction := uint16(sqlFetchFirst)
	var drivers []string
	for {
		description := make([]uint16, 1024)
		attributes := make([]uint16, 1024)
		var descriptionLength int16
		var attributeLength int16
		code, _, _ := procSQLDriversW.Call(
			env,
			uintptr(direction),
			uintptr(unsafe.Pointer(&description[0])),
			uintptr(len(description)),
			uintptr(unsafe.Pointer(&descriptionLength)),
			uintptr(unsafe.Pointer(&attributes[0])),
			uintptr(len(attributes)),
			uintptr(unsafe.Pointer(&attributeLength)),
		)
		if int16(code) == sqlNoData {
			break
		}
		if !isSQLSuccess(code) {
			return nil, odbcError("SQLDrivers failed", env, 0, 0)
		}
		name := strings.TrimSpace(syscall.UTF16ToString(description))
		if name != "" {
			drivers = append(drivers, name)
		}
		direction = sqlFetchNext
	}
	sort.Strings(drivers)
	return &DBDriversResult{Drivers: drivers}, nil
}

func queryDB(req DBQueryRequest, cfg *Config) (*DBQueryResult, *APIError) {
	sqlText := strings.TrimSpace(req.SQL)
	if sqlText == "" {
		return nil, invalidArg("sql is required")
	}
	if req.DSN != "" && req.Connection != "" {
		return nil, invalidArg("dsn and connection are mutually exclusive")
	}
	connection := strings.TrimSpace(req.Connection)
	if req.DSN != "" {
		value, ok := cfg.DSNs[req.DSN]
		if !ok {
			return nil, apiError("db_unavailable", "unknown dsn: "+req.DSN, 400)
		}
		connection = value
	}
	if connection == "" {
		return nil, apiError("db_unavailable", "dsn or connection is required", 400)
	}
	readOnly := true
	if req.ReadOnly != nil {
		readOnly = *req.ReadOnly
	}
	if !readOnly && !cfg.AllowDBWrite {
		return nil, capabilityDisabled("database write")
	}
	if readOnly && (!isReadOnlySQL(sqlText) || hasMultipleStatements(sqlText)) {
		return nil, apiError("db_read_only", "only one SELECT/WITH/SHOW/DESCRIBE/EXPLAIN/PRAGMA statement is allowed in read-only mode", 403)
	}
	maxRows := req.MaxRows
	if maxRows <= 0 {
		maxRows = 200
	}
	if maxRows > 5000 {
		maxRows = 5000
	}
	timeoutMS := req.TimeoutMS
	if timeoutMS <= 0 {
		timeoutMS = 30000
	}
	if timeoutMS > 600000 {
		timeoutMS = 600000
	}
	timeoutSeconds := timeoutMS / 1000
	if timeoutSeconds < 1 {
		timeoutSeconds = 1
	}

	env, apiErr := odbcAllocateEnv()
	if apiErr != nil {
		return nil, apiErr
	}
	defer procSQLFreeHandle.Call(sqlHandleEnv, env)
	dbc, apiErr := odbcAllocateDbc(env)
	if apiErr != nil {
		return nil, apiErr
	}
	defer func() {
		procSQLDisconnect.Call(dbc)
		procSQLFreeHandle.Call(sqlHandleDbc, dbc)
	}()

	connectionPtr, err := syscall.UTF16PtrFromString(connection)
	if err != nil {
		return nil, invalidArg("connection string contains invalid characters")
	}
	output := make([]uint16, 1024)
	var outputLength int16
	code, _, _ := procSQLDriverConnectW.Call(
		dbc,
		0,
		uintptr(unsafe.Pointer(connectionPtr)),
		sqlNTS16,
		uintptr(unsafe.Pointer(&output[0])),
		uintptr(len(output)),
		uintptr(unsafe.Pointer(&outputLength)),
		sqlDriverNoPrompt,
	)
	runtime.KeepAlive(connectionPtr)
	if !isSQLSuccess(code) {
		return nil, odbcError("ODBC connection failed", env, dbc, 0)
	}
	if readOnly {
		procSQLSetConnectAttrW.Call(dbc, sqlAttrAccessMode, sqlModeReadOnly, 0)
	}

	stmt, apiErr := odbcAllocateStmt(dbc)
	if apiErr != nil {
		return nil, apiErr
	}
	defer procSQLFreeHandle.Call(sqlHandleStmt, stmt)
	procSQLSetStmtAttrW.Call(stmt, sqlAttrQueryTimeout, uintptr(timeoutSeconds), 0)
	procSQLSetStmtAttrW.Call(stmt, sqlAttrMaxRows, uintptr(maxRows), 0)

	queryPtr, err := syscall.UTF16PtrFromString(sqlText)
	if err != nil {
		return nil, invalidArg("sql contains invalid characters")
	}
	code, _, _ = procSQLExecDirectW.Call(stmt, uintptr(unsafe.Pointer(queryPtr)), sqlNTS32)
	runtime.KeepAlive(queryPtr)
	if !isSQLSuccess(code) {
		return nil, odbcError("SQL execution failed", env, dbc, stmt)
	}

	var columnCount int16
	code, _, _ = procSQLNumResultCols.Call(stmt, uintptr(unsafe.Pointer(&columnCount)))
	if !isSQLSuccess(code) {
		return nil, odbcError("SQLNumResultCols failed", env, dbc, stmt)
	}
	result := &DBQueryResult{Columns: make([]DBColumn, 0, int(columnCount)), Rows: make([][]interface{}, 0)}
	binaryColumns := make([]bool, int(columnCount))
	for index := 0; index < int(columnCount); index++ {
		name := make([]uint16, 256)
		var nameLength int16
		dataType := int16(0)
		var columnSize uintptr
		var decimalDigits int16
		var nullable int16
		code, _, _ = procSQLDescribeColW.Call(
			stmt,
			uintptr(index+1),
			uintptr(unsafe.Pointer(&name[0])),
			uintptr(len(name)),
			uintptr(unsafe.Pointer(&nameLength)),
			uintptr(unsafe.Pointer(&dataType)),
			uintptr(unsafe.Pointer(&columnSize)),
			uintptr(unsafe.Pointer(&decimalDigits)),
			uintptr(unsafe.Pointer(&nullable)),
		)
		if !isSQLSuccess(code) {
			return nil, odbcError("SQLDescribeCol failed", env, dbc, stmt)
		}
		binary := dataType == -2 || dataType == -3 || dataType == -4
		binaryColumns[index] = binary
		result.Columns = append(result.Columns, DBColumn{
			Name:   syscall.UTF16ToString(name),
			Type:   dataType,
			Binary: binary,
		})
	}

	cellBuffer := make([]byte, sqlMaxCellBytes)
	for rowIndex := 0; rowIndex < maxRows; rowIndex++ {
		code, _, _ = procSQLFetch.Call(stmt)
		if int16(code) == sqlNoData {
			break
		}
		if !isSQLSuccess(code) {
			return nil, odbcError("SQLFetch failed", env, dbc, stmt)
		}
		row := make([]interface{}, int(columnCount))
		for columnIndex := 0; columnIndex < int(columnCount); columnIndex++ {
			value, isNull, truncated, apiErr := readODBCCell(stmt, columnIndex+1, binaryColumns[columnIndex], cellBuffer)
			if apiErr != nil {
				return nil, apiErr
			}
			if truncated {
				result.Truncated = true
			}
			if isNull {
				row[columnIndex] = nil
			} else {
				row[columnIndex] = value
			}
		}
		result.Rows = append(result.Rows, row)
	}
	result.RowCount = len(result.Rows)
	return result, nil
}

func readODBCCell(stmt uintptr, column int, binary bool, buffer []byte) (string, bool, bool, *APIError) {
	targetType := sqlCChar
	if binary {
		targetType = sqlCBinary
	}
	var indicator int
	code, _, _ := procSQLGetData.Call(
		stmt,
		uintptr(column),
		targetType,
		uintptr(unsafe.Pointer(&buffer[0])),
		uintptr(len(buffer)),
		uintptr(unsafe.Pointer(&indicator)),
	)
	if int16(code) == sqlNoData {
		return "", true, false, nil
	}
	if !isSQLSuccess(code) {
		return "", false, false, apiError("db_error", "SQLGetData failed", 400)
	}
	if indicator == sqlNullData {
		return "", true, false, nil
	}
	truncated := indicator == sqlNoTotal
	length := len(buffer)
	if indicator >= 0 {
		if indicator < length {
			length = indicator
		} else if indicator > len(buffer) {
			truncated = true
			if !binary && length > 0 {
				length = len(buffer) - 1
			}
		}
	} else if !binary && length > 0 {
		length = len(buffer) - 1
	}
	if length < 0 {
		length = 0
	}
	data := buffer[:length]
	if binary {
		return base64.StdEncoding.EncodeToString(data), false, truncated, nil
	}
	if utf8.Valid(data) {
		return string(data), false, truncated, nil
	}
	return decodeConsoleBytes(data, "ansi"), false, truncated, nil
}

// 只读模式必须同时挡住多语句：部分驱动不支持 SQL_ATTR_ACCESS_MODE，
// 仅靠首词白名单会被 "SELECT 1; DELETE ..." 绕过。
func hasMultipleStatements(sqlText string) bool {
	inSingle := false
	inDouble := false
	for index := 0; index < len(sqlText); index++ {
		char := sqlText[index]
		if inSingle {
			if char == '\'' {
				if index+1 < len(sqlText) && sqlText[index+1] == '\'' {
					index++
				} else {
					inSingle = false
				}
			}
			continue
		}
		if inDouble {
			if char == '"' {
				inDouble = false
			}
			continue
		}
		switch char {
		case '\'':
			inSingle = true
		case '"':
			inDouble = true
		case ';':
			if strings.TrimSpace(sqlText[index+1:]) != "" {
				return true
			}
		}
	}
	return false
}

func isReadOnlySQL(sqlText string) bool {
	trimmed := strings.TrimSpace(sqlText)
	for {
		switch {
		case strings.HasPrefix(trimmed, "--"):
			index := strings.IndexByte(trimmed, '\n')
			if index < 0 {
				return false
			}
			trimmed = strings.TrimSpace(trimmed[index+1:])
		case strings.HasPrefix(trimmed, "/*"):
			index := strings.Index(trimmed, "*/")
			if index < 0 {
				return false
			}
			trimmed = strings.TrimSpace(trimmed[index+2:])
		default:
			goto done
		}
	}
done:
	upper := strings.ToUpper(trimmed)
	if upper == "" {
		return false
	}
	for _, prefix := range []string{"SELECT", "WITH", "SHOW", "DESCRIBE", "DESC", "EXPLAIN", "PRAGMA"} {
		if upper == prefix || strings.HasPrefix(upper, prefix+" ") || strings.HasPrefix(upper, prefix+"\t") || strings.HasPrefix(upper, prefix+"\n") || strings.HasPrefix(upper, prefix+"(") {
			return true
		}
	}
	return false
}
