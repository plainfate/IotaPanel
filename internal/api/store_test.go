// SPDX-License-Identifier: GPL-3.0-or-later
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// IotaPanel is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// IotaPanel is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with IotaPanel.  If not, see <https://www.gnu.org/licenses/>.

package api

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"testing"
)

// buildPluginPackage 构造内存中的 gzip+tar 插件包：entries 为 相对路径 -> 内容。
func buildPluginPackage(t *testing.T, entries map[string]string) []byte {
	t.Helper()
	var buf bytes.Buffer
	gz := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gz)
	for name, content := range entries {
		if err := tw.WriteHeader(&tar.Header{Name: name, Mode: 0o644, Size: int64(len(content)), Typeflag: tar.TypeReg}); err != nil {
			t.Fatal(err)
		}
		if _, err := tw.Write([]byte(content)); err != nil {
			t.Fatal(err)
		}
	}
	if err := tw.Close(); err != nil {
		t.Fatal(err)
	}
	if err := gz.Close(); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func TestUnpackPluginPackageValid(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"demo/manifest.yaml": "name: demo\n",
		"demo/bin/run":       "#!/bin/sh\n",
	})
	name, files, err := unpackPluginPackage(data)
	if err != nil {
		t.Fatalf("valid package rejected: %v", err)
	}
	if name != "demo" {
		t.Fatalf("top dir = %q, want demo", name)
	}
	if _, ok := files["manifest.yaml"]; !ok {
		t.Fatal("manifest.yaml missing from extracted files")
	}
}

func TestUnpackPluginPackageRejectsTraversal(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"../../etc/cron.d/evil": "x",
	})
	if _, _, err := unpackPluginPackage(data); err == nil {
		t.Fatal("path traversal accepted")
	}
}

func TestUnpackPluginPackageRejectsMultipleTopDirs(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"a/manifest.yaml": "name: a\n",
		"b/manifest.yaml": "name: b\n",
	})
	if _, _, err := unpackPluginPackage(data); err == nil {
		t.Fatal("multiple top-level dirs accepted")
	}
}

func TestUnpackPluginPackageRequiresManifest(t *testing.T) {
	data := buildPluginPackage(t, map[string]string{
		"demo/readme.txt": "hi",
	})
	if _, _, err := unpackPluginPackage(data); err == nil {
		t.Fatal("package without manifest.yaml accepted")
	}
}

func TestUnpackPluginPackageRejectsNonGzip(t *testing.T) {
	if _, _, err := unpackPluginPackage([]byte("this is not gzip")); err == nil {
		t.Fatal("non-gzip payload accepted")
	}
}
