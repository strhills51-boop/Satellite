// Package web embeds the built UI (web/dist).
package web

import "embed"

// Dist holds the compiled frontend.
//
//go:embed all:dist
var Dist embed.FS
