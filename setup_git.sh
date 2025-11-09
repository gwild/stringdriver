#!/bin/bash
# Setup script for stringdriver repository

set -e

cd "$(dirname "$0")"

echo "Initializing git repository..."
git init

echo "Adding files..."
git add -A

echo "Creating initial commit..."
git commit -m "Initial commit: Rust GUI binaries for string driver"

echo "Adding remote..."
git remote add origin git@github.com:gwild/stringdriver.git || git remote set-url origin git@github.com:gwild/stringdriver.git

echo "Setting branch to main..."
git branch -M main

echo "Ready to push! Run: git push -u origin main"

