package cmd

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/spf13/cobra"
)

const githubRepo = "thinkany-ai/codeany"

var updateCmd = &cobra.Command{
	Use:   "update",
	Short: "Update codeany to the latest version",
	Long:  "Check GitHub Releases for a newer version and self-update if available.",
	RunE:  runUpdate,
}

func init() {
	rootCmd.AddCommand(updateCmd)
}

type ghRelease struct {
	TagName string `json:"tag_name"`
	Assets  []struct {
		Name               string `json:"name"`
		BrowserDownloadURL string `json:"browser_download_url"`
	} `json:"assets"`
}

func runUpdate(cmd *cobra.Command, args []string) error {
	current := appVersion
	fmt.Printf("Current version : %s\n", current)
	fmt.Printf("Checking latest release from GitHub...\n")

	rel, err := fetchLatestRelease()
	if err != nil {
		return fmt.Errorf("checking for updates: %w", err)
	}

	latest := rel.TagName
	fmt.Printf("Latest version  : %s\n", latest)

	if latest == current || latest == "v"+strings.TrimPrefix(current, "v") {
		fmt.Println("✓ Already up to date.")
		return nil
	}

	// Find asset for current platform
	platform := fmt.Sprintf("%s_%s", runtime.GOOS, runtime.GOARCH)
	ext := ""
	if runtime.GOOS == "windows" {
		ext = ".exe"
	}
	assetName := fmt.Sprintf("codeany_%s%s", platform, ext)

	var downloadURL string
	for _, a := range rel.Assets {
		if a.Name == assetName {
			downloadURL = a.BrowserDownloadURL
			break
		}
	}
	if downloadURL == "" {
		return fmt.Errorf("no binary found for platform %s in release %s", platform, latest)
	}

	fmt.Printf("Downloading %s...\n", latest)

	// Download to temp file
	tmpFile, err := os.CreateTemp("", "codeany-update-*")
	if err != nil {
		return fmt.Errorf("creating temp file: %w", err)
	}
	tmpPath := tmpFile.Name()
	defer os.Remove(tmpPath)

	resp, err := http.Get(downloadURL) //nolint:gosec
	if err != nil {
		return fmt.Errorf("downloading: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("download failed: HTTP %d", resp.StatusCode)
	}

	if _, err := io.Copy(tmpFile, resp.Body); err != nil {
		return fmt.Errorf("saving download: %w", err)
	}
	tmpFile.Close()

	if err := os.Chmod(tmpPath, 0755); err != nil {
		return fmt.Errorf("chmod: %w", err)
	}

	// Get path of current executable
	exePath, err := os.Executable()
	if err != nil {
		return fmt.Errorf("getting executable path: %w", err)
	}
	exePath, err = filepath.EvalSymlinks(exePath)
	if err != nil {
		return fmt.Errorf("resolving symlinks: %w", err)
	}

	// Check if writable (user bin = no sudo needed)
	if err := checkWritable(exePath); err != nil {
		return fmt.Errorf("cannot update %s (permission denied). Try: sudo codeany update", exePath)
	}

	// Replace binary atomically: rename old, move new, remove old
	oldPath := exePath + ".old"
	if err := os.Rename(exePath, oldPath); err != nil {
		return fmt.Errorf("backing up old binary: %w", err)
	}
	if err := os.Rename(tmpPath, exePath); err != nil {
		// Restore old binary on failure
		_ = os.Rename(oldPath, exePath)
		return fmt.Errorf("replacing binary: %w", err)
	}
	os.Remove(oldPath)

	fmt.Printf("✓ Updated to %s (%s)\n", latest, exePath)
	return nil
}

func fetchLatestRelease() (*ghRelease, error) {
	url := fmt.Sprintf("https://api.github.com/repos/%s/releases/latest", githubRepo)
	resp, err := http.Get(url) //nolint:gosec
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("GitHub API returned HTTP %d", resp.StatusCode)
	}

	var rel ghRelease
	if err := json.NewDecoder(resp.Body).Decode(&rel); err != nil {
		return nil, err
	}
	return &rel, nil
}

func checkWritable(path string) error {
	f, err := os.OpenFile(path, os.O_WRONLY, 0)
	if err != nil {
		return err
	}
	f.Close()
	return nil
}
