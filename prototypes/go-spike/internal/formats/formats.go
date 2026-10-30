// Package formats parses Bethesda plugin headers, BSA/BA2 archive manifests and DDS headers.
package formats

import (
	"bufio"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"strings"
)

var le = binary.LittleEndian

// ---------------------------------------------------------------- plugins

// Plugin holds TES4 header data.
type Plugin struct {
	Name        string            `json:"name"`
	Master      bool              `json:"master"`   // ESM flag 0x1
	Light       bool              `json:"light"`    // ESL flag 0x200
	Localized   bool              `json:"localized"`
	Version     float32           `json:"version"`
	NumRecords  uint32            `json:"numRecords"`
	NextObject  uint32            `json:"nextObjectId"`
	Author      string            `json:"author,omitempty"`
	Description string            `json:"description,omitempty"`
	Masters     []string          `json:"masters"`
	Records     map[string]uint32 `json:"records,omitempty"` // filled by ScanRecords
	RecordTotal uint32            `json:"recordTotal,omitempty"`
	Error       string            `json:"error,omitempty"`
}

// Kind returns ESM/ESP/ESL label.
func (p *Plugin) Kind() string {
	ext := strings.ToLower(p.Name[max(0, len(p.Name)-4):])
	switch {
	case ext == ".esl" || p.Light:
		return "ESL"
	case ext == ".esm" || p.Master:
		return "ESM"
	}
	return "ESP"
}

// ReadPluginHeader reads the TES4 record. Skyrim/FO4 use 24-byte record headers.
func ReadPluginHeader(path, name string) *Plugin {
	p := &Plugin{Name: name}
	f, err := os.Open(path)
	if err != nil {
		p.Error = err.Error()
		return p
	}
	defer f.Close()
	var hdr [24]byte
	if _, err := io.ReadFull(f, hdr[:]); err != nil {
		p.Error = "короткий файл"
		return p
	}
	sig := string(hdr[0:4])
	if sig != "TES4" && sig != "TES3" {
		p.Error = "не TES4: " + sig
		return p
	}
	size := le.Uint32(hdr[4:])
	flags := le.Uint32(hdr[8:])
	p.Master = flags&0x1 != 0
	p.Localized = flags&0x80 != 0
	p.Light = flags&0x200 != 0
	if size > 64<<20 {
		p.Error = "TES4 слишком большой"
		return p
	}
	data := make([]byte, size)
	if _, err := io.ReadFull(f, data); err != nil {
		p.Error = "обрезанный TES4"
		return p
	}
	for o := 0; o+6 <= len(data); {
		ss := string(data[o : o+4])
		sl := int(le.Uint16(data[o+4:]))
		o += 6
		if o+sl > len(data) {
			break
		}
		v := data[o : o+sl]
		switch ss {
		case "HEDR":
			if len(v) >= 12 {
				p.Version = float32frombits(le.Uint32(v))
				p.NumRecords = le.Uint32(v[4:])
				p.NextObject = le.Uint32(v[8:])
			}
		case "CNAM":
			p.Author = zstr(v)
		case "SNAM":
			p.Description = zstr(v)
		case "MAST":
			p.Masters = append(p.Masters, zstr(v))
		}
		o += sl
	}
	return p
}

// ScanRecords walks all record headers (without decompressing) and counts signatures.
func ScanRecords(path string, p *Plugin) error {
	f, err := os.Open(path)
	if err != nil {
		return err
	}
	defer f.Close()
	r := bufio.NewReaderSize(f, 1<<20)
	counts := map[string]uint32{}
	var hdr [24]byte
	total := uint32(0)
	for {
		if _, err := io.ReadFull(r, hdr[:]); err != nil {
			if errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) {
				break
			}
			return err
		}
		sig := string(hdr[0:4])
		if sig == "GRUP" {
			continue // group contents follow immediately
		}
		size := int(le.Uint32(hdr[4:]))
		if _, err := r.Discard(size); err != nil {
			break
		}
		if sig != "TES4" {
			counts[sig]++
			total++
		}
	}
	p.Records = counts
	p.RecordTotal = total
	return nil
}

func zstr(b []byte) string {
	if i := indexByte(b, 0); i >= 0 {
		b = b[:i]
	}
	return string(b)
}

func indexByte(b []byte, c byte) int {
	for i, x := range b {
		if x == c {
			return i
		}
	}
	return -1
}

// ---------------------------------------------------------------- archives

// ArchiveEntry is one file inside a BSA/BA2.
type ArchiveEntry struct {
	Path string // lowercased, forward slashes
	Size uint64
	VRAM uint64 // known for BA2 DX10 textures
}

// ReadArchive dispatches on magic.
func ReadArchive(path string) (kind string, entries []ArchiveEntry, err error) {
	f, err := os.Open(path)
	if err != nil {
		return "", nil, err
	}
	defer f.Close()
	var magic [4]byte
	if _, err := io.ReadFull(f, magic[:]); err != nil {
		return "", nil, err
	}
	if _, err := f.Seek(0, io.SeekStart); err != nil {
		return "", nil, err
	}
	switch string(magic[:]) {
	case "BSA\x00":
		e, err := readBSA(f)
		return "BSA", e, err
	case "BTDX":
		e, k, err := readBA2(f)
		return k, e, err
	}
	return "", nil, fmt.Errorf("неизвестный архив %q", magic)
}

func readBSA(f *os.File) ([]ArchiveEntry, error) {
	r := bufio.NewReaderSize(f, 1<<20)
	var h [36]byte
	if _, err := io.ReadFull(r, h[:]); err != nil {
		return nil, err
	}
	ver := le.Uint32(h[4:])
	aflags := le.Uint32(h[12:])
	folders := int(le.Uint32(h[16:]))
	files := int(le.Uint32(h[20:]))
	fileNameLen := int(le.Uint32(h[28:]))
	if ver != 103 && ver != 104 && ver != 105 {
		return nil, fmt.Errorf("BSA версии %d не поддерживается", ver)
	}
	if folders > 1<<22 || files > 1<<24 {
		return nil, errors.New("BSA: подозрительные размеры")
	}
	recSize := 16
	if ver == 105 {
		recSize = 24
	}
	counts := make([]int, folders)
	rec := make([]byte, recSize)
	for i := range counts {
		if _, err := io.ReadFull(r, rec); err != nil {
			return nil, err
		}
		counts[i] = int(le.Uint32(rec[8:]))
	}
	hasDirNames := aflags&0x1 != 0
	hasFileNames := aflags&0x2 != 0
	compressedDefault := aflags&0x4 != 0
	_ = compressedDefault
	type fe struct {
		dir  string
		size uint32
	}
	list := make([]fe, 0, files)
	var fr [16]byte
	for i := 0; i < folders; i++ {
		dir := ""
		if hasDirNames {
			n, err := r.ReadByte()
			if err != nil {
				return nil, err
			}
			b := make([]byte, n)
			if _, err := io.ReadFull(r, b); err != nil {
				return nil, err
			}
			dir = zstr(b)
		}
		for j := 0; j < counts[i]; j++ {
			if _, err := io.ReadFull(r, fr[:]); err != nil {
				return nil, err
			}
			list = append(list, fe{dir: dir, size: le.Uint32(fr[8:]) & 0x3FFFFFFF})
		}
	}
	out := make([]ArchiveEntry, 0, len(list))
	if !hasFileNames {
		return out, nil
	}
	names := make([]byte, fileNameLen)
	if _, err := io.ReadFull(r, names); err != nil {
		return nil, err
	}
	o := 0
	for _, e := range list {
		end := o
		for end < len(names) && names[end] != 0 {
			end++
		}
		name := string(names[o:end])
		o = end + 1
		p := name
		if e.dir != "" && e.dir != "." {
			p = e.dir + "/" + name
		}
		out = append(out, ArchiveEntry{Path: normPath(p), Size: uint64(e.size)})
	}
	return out, nil
}

func readBA2(f *os.File) ([]ArchiveEntry, string, error) {
	var h [24]byte
	if _, err := io.ReadFull(f, h[:]); err != nil {
		return nil, "", err
	}
	ver := le.Uint32(h[4:])
	typ := string(h[8:12])
	count := int(le.Uint32(h[12:]))
	nameOff := int64(le.Uint64(h[16:]))
	if count > 1<<24 {
		return nil, "", errors.New("BA2: подозрительное число файлов")
	}
	extra := 0
	switch ver {
	case 2:
		extra = 8
	case 3:
		extra = 12
	}
	r := bufio.NewReaderSize(f, 1<<20)
	if extra > 0 {
		if _, err := r.Discard(extra); err != nil {
			return nil, "", err
		}
	}
	out := make([]ArchiveEntry, count)
	switch typ {
	case "GNRL":
		var rec [36]byte
		for i := 0; i < count; i++ {
			if _, err := io.ReadFull(r, rec[:]); err != nil {
				return nil, "", err
			}
			out[i].Size = uint64(le.Uint32(rec[28:]))
			if out[i].Size == 0 {
				out[i].Size = uint64(le.Uint32(rec[24:]))
			}
		}
	case "DX10":
		var rec [24]byte
		var chunk [24]byte
		for i := 0; i < count; i++ {
			if _, err := io.ReadFull(r, rec[:]); err != nil {
				return nil, "", err
			}
			nChunks := int(rec[13])
			height := uint32(le.Uint16(rec[16:]))
			width := uint32(le.Uint16(rec[18:]))
			mips := uint32(rec[20])
			format := uint32(rec[21])
			cube := rec[22] == 1
			var full uint64
			for c := 0; c < nChunks; c++ {
				if _, err := io.ReadFull(r, chunk[:]); err != nil {
					return nil, "", err
				}
				full += uint64(le.Uint32(chunk[12:]))
			}
			out[i].Size = full
			faces := uint32(1)
			if cube {
				faces = 6
			}
			out[i].VRAM = TextureBytes(width, height, 1, mips, format, faces)
		}
	default:
		return nil, "", fmt.Errorf("BA2 тип %q не поддерживается", typ)
	}
	if _, err := f.Seek(nameOff, io.SeekStart); err != nil {
		return nil, "", err
	}
	r = bufio.NewReaderSize(f, 1<<20)
	var lb [2]byte
	for i := 0; i < count; i++ {
		if _, err := io.ReadFull(r, lb[:]); err != nil {
			return nil, "", err
		}
		b := make([]byte, le.Uint16(lb[:]))
		if _, err := io.ReadFull(r, b); err != nil {
			return nil, "", err
		}
		out[i].Path = normPath(string(b))
	}
	return out, "BA2-" + typ, nil
}

func normPath(p string) string {
	return strings.ToLower(strings.Trim(strings.ReplaceAll(p, `\`, "/"), "/"))
}

// ---------------------------------------------------------------- DDS

// DDSInfo is the decoded DDS header.
type DDSInfo struct {
	Width, Height, Depth, Mips, Faces uint32
	Format                            string
	VRAM                              uint64
	Known                             bool
}

// ReadDDSHeader reads a DDS header (148 bytes max) and estimates VRAM.
func ReadDDSHeader(path string) DDSInfo {
	f, err := os.Open(path)
	if err != nil {
		return DDSInfo{}
	}
	defer f.Close()
	var b [148]byte
	n, _ := io.ReadFull(f, b[:])
	return ParseDDS(b[:n])
}

// ParseDDS parses DDS header bytes.
func ParseDDS(b []byte) DDSInfo {
	var d DDSInfo
	if len(b) < 128 || string(b[0:4]) != "DDS " {
		return d
	}
	d.Height = le.Uint32(b[12:])
	d.Width = le.Uint32(b[16:])
	d.Depth = max(1, le.Uint32(b[24:]))
	d.Mips = max(1, le.Uint32(b[28:]))
	pfFlags := le.Uint32(b[80:])
	fourCC := string(b[84:88])
	bits := le.Uint32(b[88:])
	caps2 := le.Uint32(b[112:])
	d.Faces = 1
	if caps2&0x200 != 0 {
		d.Faces = 6
	}
	var dxgi uint32
	switch {
	case pfFlags&0x4 != 0 && fourCC == "DX10":
		if len(b) < 148 {
			return d
		}
		dxgi = le.Uint32(b[128:])
		if le.Uint32(b[140:])&0x4 != 0 { // misc flag TEXTURECUBE
			d.Faces = 6 * max(1, le.Uint32(b[136:]))
		}
	case pfFlags&0x4 != 0:
		switch fourCC {
		case "DXT1":
			dxgi = 71
		case "DXT2", "DXT3":
			dxgi = 74
		case "DXT4", "DXT5":
			dxgi = 77
		case "ATI1", "BC4U":
			dxgi = 80
		case "ATI2", "BC5U":
			dxgi = 83
		default:
			d.Format = fourCC
			return d
		}
	default:
		if bits == 0 {
			return d
		}
		d.Format = fmt.Sprintf("RGB%d", bits)
		d.VRAM = uncompressedBytes(d.Width, d.Height, d.Depth, d.Mips, bits) * uint64(d.Faces)
		d.Known = true
		return d
	}
	d.Format = DXGIName(dxgi)
	d.VRAM = TextureBytes(d.Width, d.Height, d.Depth, d.Mips, dxgi, d.Faces)
	d.Known = d.VRAM > 0
	return d
}

func uncompressedBytes(w, h, depth, mips, bits uint32) uint64 {
	var t uint64
	for m := uint32(0); m < mips; m++ {
		mw, mh, md := max(1, w>>m), max(1, h>>m), max(1, depth>>m)
		t += uint64(mw) * uint64(mh) * uint64(md) * uint64(bits) / 8
	}
	return t
}

// TextureBytes estimates GPU memory for a DXGI-format texture with a mip chain.
func TextureBytes(w, h, depth, mips, dxgi, faces uint32) uint64 {
	block, bpp := dxgiBlock(dxgi)
	if block == 0 && bpp == 0 {
		return 0
	}
	if mips == 0 {
		mips = 1
	}
	if depth == 0 {
		depth = 1
	}
	var t uint64
	for m := uint32(0); m < mips; m++ {
		mw, mh, md := max(1, w>>m), max(1, h>>m), max(1, depth>>m)
		if block > 0 {
			bw, bh := (mw+3)/4, (mh+3)/4
			t += uint64(bw) * uint64(bh) * uint64(block) * uint64(md)
		} else {
			t += uint64(mw) * uint64(mh) * uint64(md) * uint64(bpp) / 8
		}
	}
	return t * uint64(max(1, faces))
}

// dxgiBlock returns (bytes per 4x4 block, 0) for BC formats or (0, bits per pixel).
func dxgiBlock(f uint32) (uint32, uint32) {
	switch {
	case f >= 70 && f <= 72, f >= 79 && f <= 81: // BC1, BC4
		return 8, 0
	case f >= 73 && f <= 78, f >= 82 && f <= 84, f >= 94 && f <= 99: // BC2/3/5/6H/7
		return 16, 0
	}
	switch f {
	case 2, 3, 4:
		return 0, 128
	case 6, 7, 8:
		return 0, 96
	case 10, 11, 12, 13, 14, 16, 17, 18:
		return 0, 64
	case 24, 25, 26, 27, 28, 29, 30, 31, 32, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 87, 88, 89, 90, 91, 92, 93:
		return 0, 32
	case 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 85, 86, 115:
		return 0, 16
	case 60, 61, 62, 63, 64, 65:
		return 0, 8
	}
	return 0, 0
}

// DXGIName returns a short name for common DXGI formats.
func DXGIName(f uint32) string {
	switch {
	case f >= 70 && f <= 72:
		return "BC1"
	case f >= 73 && f <= 75:
		return "BC2"
	case f >= 76 && f <= 78:
		return "BC3"
	case f >= 79 && f <= 81:
		return "BC4"
	case f >= 82 && f <= 84:
		return "BC5"
	case f >= 94 && f <= 96:
		return "BC6H"
	case f >= 97 && f <= 99:
		return "BC7"
	case f == 28 || f == 29 || f == 87 || f == 91:
		return "RGBA8"
	}
	return fmt.Sprintf("DXGI%d", f)
}

func float32frombits(u uint32) float32 { return math.Float32frombits(u) }
