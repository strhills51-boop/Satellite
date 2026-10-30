// Package mo2 reads Mod Organizer 2 instance and profile files.
package mo2

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// Instance describes resolved MO2 instance paths.
type Instance struct {
	IniPath       string   `json:"iniPath"`
	BaseDir       string   `json:"baseDir"`
	GameName      string   `json:"gameName"`
	GamePath      string   `json:"gamePath"`
	ModsDir       string   `json:"modsDir"`
	ExtraModDirs  []string `json:"extraModDirs"`
	OverwriteDir  string   `json:"overwriteDir"`
	ProfilesDir   string   `json:"profilesDir"`
	Profile       string   `json:"profile"`
	Profiles      []string `json:"profiles"`
}

// ProfileDir returns the active profile directory.
func (in *Instance) ProfileDir() string { return filepath.Join(in.ProfilesDir, in.Profile) }

// ModRoots returns all mod roots, primary first.
func (in *Instance) ModRoots() []string {
	out := []string{in.ModsDir}
	for _, d := range in.ExtraModDirs {
		if d != "" && !strings.EqualFold(filepath.Clean(d), filepath.Clean(in.ModsDir)) {
			out = append(out, d)
		}
	}
	return out
}

// ParseIni parses a Qt-style ini file into section -> key -> value (keys lowercased).
func ParseIni(data []byte) map[string]map[string]string {
	out := map[string]map[string]string{}
	sec := ""
	out[sec] = map[string]string{}
	sc := bufio.NewScanner(bytes.NewReader(stripBOM(data)))
	sc.Buffer(make([]byte, 1024*1024), 16*1024*1024)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || line[0] == ';' || line[0] == '#' {
			continue
		}
		if line[0] == '[' && strings.HasSuffix(line, "]") {
			sec = strings.ToLower(line[1 : len(line)-1])
			if out[sec] == nil {
				out[sec] = map[string]string{}
			}
			continue
		}
		k, v, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		out[sec][strings.ToLower(strings.TrimSpace(k))] = strings.TrimSpace(v)
	}
	return out
}

func stripBOM(b []byte) []byte { return bytes.TrimPrefix(b, []byte{0xEF, 0xBB, 0xBF}) }

// unqt removes Qt wrappers like @ByteArray(...) and quotes, and unescapes backslashes.
func unqt(v string) string {
	v = strings.TrimSpace(v)
	if strings.HasPrefix(v, "@ByteArray(") && strings.HasSuffix(v, ")") {
		v = v[len("@ByteArray(") : len(v)-1]
	}
	if len(v) >= 2 && v[0] == '"' && v[len(v)-1] == '"' {
		v = v[1 : len(v)-1]
	}
	v = strings.ReplaceAll(v, `\\`, `\`)
	return v
}

// LoadInstance resolves an MO2 instance from a ModOrganizer.ini path or an instance directory.
func LoadInstance(p string) (*Instance, error) {
	p = strings.TrimSpace(strings.Trim(p, `"`))
	if p == "" {
		return nil, errors.New("путь не указан")
	}
	st, err := os.Stat(p)
	if err != nil {
		return nil, err
	}
	if abs, err := filepath.Abs(p); err == nil {
		p = abs
	}
	iniPath := p
	if st.IsDir() {
		iniPath = filepath.Join(p, "ModOrganizer.ini")
	}
	data, err := os.ReadFile(iniPath)
	if err != nil {
		return nil, fmt.Errorf("не найден ModOrganizer.ini: %w", err)
	}
	ini := ParseIni(data)
	g, s := ini["general"], ini["settings"]
	if s == nil {
		s = map[string]string{}
	}
	in := &Instance{IniPath: iniPath}
	instDir := filepath.Dir(iniPath)
	base := unqt(s["base_directory"])
	if base == "" {
		base = instDir
	}
	in.BaseDir = filepath.Clean(base)
	resolve := func(key, def string) string {
		v := unqt(s[key])
		if v == "" {
			v = filepath.Join(in.BaseDir, def)
		}
		v = strings.ReplaceAll(v, "%BASE_DIR%", in.BaseDir)
		v = filepath.FromSlash(v)
		if !filepath.IsAbs(v) {
			v = filepath.Join(instDir, v)
		}
		return filepath.Clean(v)
	}
	in.ModsDir = resolve("mod_directory", "mods")
	in.OverwriteDir = resolve("overwrite_directory", "overwrite")
	in.ProfilesDir = resolve("profiles_directory", "profiles")
	if g != nil {
		in.GameName = unqt(g["gamename"])
		in.GamePath = filepath.FromSlash(unqt(g["gamepath"]))
		in.Profile = unqt(g["selected_profile"])
	}
	ents, _ := os.ReadDir(in.ProfilesDir)
	for _, e := range ents {
		if e.IsDir() {
			in.Profiles = append(in.Profiles, e.Name())
		}
	}
	if in.Profile == "" && len(in.Profiles) > 0 {
		in.Profile = in.Profiles[0]
	}
	return in, nil
}

// Entry is one line of modlist.txt.
type Entry struct {
	Name      string `json:"name"`
	Enabled   bool   `json:"enabled"`
	Separator bool   `json:"separator"`
	Unmanaged bool   `json:"unmanaged"` // '*' lines (DLC / foreign)
}

const sepSuffix = "_separator"

// DisplayName strips the _separator suffix.
func (e Entry) DisplayName() string {
	if e.Separator {
		return strings.TrimSuffix(e.Name, sepSuffix)
	}
	return e.Name
}

// ParseModlist parses modlist.txt and returns entries in MO2 display order
// (lowest priority first; i.e. reversed file order).
func ParseModlist(data []byte) []Entry {
	var fileOrder []Entry
	sc := bufio.NewScanner(bytes.NewReader(stripBOM(data)))
	sc.Buffer(make([]byte, 1024*1024), 16*1024*1024)
	for sc.Scan() {
		line := strings.TrimRight(sc.Text(), "\r\n ")
		if line == "" || line[0] == '#' {
			continue
		}
		e := Entry{}
		switch line[0] {
		case '+':
			e.Enabled = true
		case '-':
		case '*':
			e.Unmanaged, e.Enabled = true, true
		default:
			continue
		}
		e.Name = line[1:]
		if strings.HasSuffix(e.Name, sepSuffix) {
			e.Separator = true
			e.Enabled = false
		}
		fileOrder = append(fileOrder, e)
	}
	out := make([]Entry, len(fileOrder))
	for i, e := range fileOrder {
		out[len(fileOrder)-1-i] = e
	}
	return out
}

// SerializeModlist writes entries (display order) back into MO2 modlist.txt format.
func SerializeModlist(entries []Entry) []byte {
	var b bytes.Buffer
	b.WriteString("# This file was automatically generated by Mod Organizer.\r\n")
	for i := len(entries) - 1; i >= 0; i-- {
		e := entries[i]
		switch {
		case e.Unmanaged:
			b.WriteByte('*')
		case e.Separator:
			b.WriteByte('-')
		case e.Enabled:
			b.WriteByte('+')
		default:
			b.WriteByte('-')
		}
		b.WriteString(e.Name)
		b.WriteString("\r\n")
	}
	return b.Bytes()
}

// PluginLine is one entry from plugins.txt / loadorder.txt.
type PluginLine struct {
	Name    string `json:"name"`
	Enabled bool   `json:"enabled"`
}

// ParsePlugins parses plugins.txt (lines prefixed with '*' are enabled).
func ParsePlugins(data []byte) []PluginLine {
	var out []PluginLine
	for _, l := range lines(data) {
		if strings.HasPrefix(l, "*") {
			out = append(out, PluginLine{Name: l[1:], Enabled: true})
		} else {
			out = append(out, PluginLine{Name: l})
		}
	}
	return out
}

// ParseLoadorder parses loadorder.txt.
func ParseLoadorder(data []byte) []string { return lines(data) }

func lines(data []byte) []string {
	var out []string
	sc := bufio.NewScanner(bytes.NewReader(stripBOM(data)))
	for sc.Scan() {
		l := strings.TrimSpace(sc.Text())
		if l == "" || l[0] == '#' {
			continue
		}
		out = append(out, l)
	}
	return out
}

// ModMeta is the subset of a mod's meta.ini we use.
type ModMeta struct {
	ModID     string `json:"modId,omitempty"`
	Version   string `json:"version,omitempty"`
	Newest    string `json:"newest,omitempty"`
	URL       string `json:"url,omitempty"`
	GameName  string `json:"gameName,omitempty"`
	Install   string `json:"install,omitempty"`
	Notes     string `json:"notes,omitempty"`
	Color     string `json:"color,omitempty"` // separators: #rrggbb
	Category  string `json:"category,omitempty"`
}

// ReadModMeta reads <dir>/meta.ini, returns zero value when absent.
func ReadModMeta(dir string) ModMeta {
	data, err := os.ReadFile(filepath.Join(dir, "meta.ini"))
	if err != nil {
		return ModMeta{}
	}
	g := ParseIni(data)["general"]
	if g == nil {
		return ModMeta{}
	}
	m := ModMeta{
		ModID:    unqt(g["modid"]),
		Version:  unqt(g["version"]),
		Newest:   unqt(g["newestversion"]),
		URL:      unqt(g["url"]),
		GameName: unqt(g["gamename"]),
		Install:  unqt(g["installationfile"]),
		Category: unqt(g["category"]),
	}
	if n := unqt(g["notes"]); n != "" {
		m.Notes = n
	}
	if m.ModID == "0" {
		m.ModID = ""
	}
	m.Color = DecodeQColor(g["color"])
	return m
}

// DecodeQColor decodes MO2's `color=@Variant(\0\0\0\x43\x1\xff\xff\x80\x80...)` into #rrggbb.
func DecodeQColor(v string) string {
	v = strings.TrimSpace(v)
	if strings.HasPrefix(v, "#") && len(v) == 7 {
		return strings.ToLower(v)
	}
	if !strings.HasPrefix(v, "@Variant(") || !strings.HasSuffix(v, ")") {
		return ""
	}
	raw := unescapeC(v[len("@Variant(") : len(v)-1])
	if len(raw) < 4 || raw[3] != 0x43 { // QMetaType::QColor = 67
		return ""
	}
	p := raw[4:]
	if len(p) > 1 && p[0] == 0 && p[1] != 0 { // optional isNull byte
		p = p[1:]
	}
	if len(p) < 9 || p[0] == 0 { // spec Invalid
		return ""
	}
	r, g, b := p[3], p[5], p[7] // spec, a16, r16, g16, b16 (big endian, take high byte)
	return fmt.Sprintf("#%02x%02x%02x", r, g, b)
}

func unescapeC(s string) []byte {
	var out []byte
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c != '\\' || i+1 >= len(s) {
			out = append(out, c)
			continue
		}
		i++
		switch s[i] {
		case '0':
			out = append(out, 0)
		case 'x':
			j := i + 1
			for j < len(s) && j < i+3 && isHex(s[j]) {
				j++
			}
			n, _ := strconv.ParseUint(s[i+1:j], 16, 8)
			out = append(out, byte(n))
			i = j - 1
		case 'n':
			out = append(out, '\n')
		case 't':
			out = append(out, '\t')
		case 'r':
			out = append(out, '\r')
		default:
			out = append(out, s[i])
		}
	}
	return out
}

func isHex(c byte) bool {
	return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')
}

// WriteFileAtomic writes via temp file + rename.
func WriteFileAtomic(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
