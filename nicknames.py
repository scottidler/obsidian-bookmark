#!/usr/bin/env python3

import ruamel.yaml
from collections import defaultdict

yaml = ruamel.yaml.YAML()

# Load the YAML file
with open('nicknames.yml', 'r') as file:
    data = yaml.load(file)

def count_matches(original, nickname):
    original = original.lower().replace('_', '')
    nickname = nickname.lower()
    match_count = 0
    original_letters = list(original)
    
    for letter in nickname:
        if letter in original_letters:
            match_count += 1
            original_letters.remove(letter)
    
    return match_count

# Process the nicknames and sort them by the number of matches
processed_data = defaultdict(list)
for key, values in data.items():
    original_word = key.replace('_', ' ').lower()
    matches = [(name, count_matches(original_word, name)) for name in values]
    sorted_matches = sorted(matches, key=lambda x: x[1], reverse=True)
    processed_data[key] = [f"{name} ({match_count})" for name, match_count in sorted_matches]

# Print the processed and sorted data to stdout
for key, values in processed_data.items():
    print(f"{key}:")
    for value in values:
        print(f"  - {value}")
