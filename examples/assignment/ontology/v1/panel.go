package view
func Bind(value string, enabled bool) string {
 if enabled { return value }; return ""
}
