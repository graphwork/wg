# Investigation of wg Research Tool Limitations

## Overview

This document investigates the current limitations of research tools in the wg environment, specifically focusing on source tracking and citation capabilities.

## Current State Analysis

Based on examination of the wg environment, the following issues have been identified:

### 1. Source Tracking Limitation

The `deep_research` and `web_search` tools do not preserve bibliographic information or source citations in their outputs. This creates a fundamental gap in traceability for users who need to verify or cite sources.

### 2. Environment Variable Discovery

There appears to be no clear mechanism for discovering conversation data locations through environment variables or configuration options. This makes it difficult for users to:
- Locate their conversation history
- Access research logs
- Integrate with external systems

### 3. Data Persistence Issues

While research tools generate output files, there's no clear indication of where these are stored or how they can be accessed programmatically.

## Technical Findings

### File System Location
Research outputs are stored in:
```
/home/erik/workgraph/.wg/nex-sessions/tool-outputs/
```

However, these files contain only the synthesized content without source metadata.

### Conversation Data Location
The conversation data is likely stored in:
```
/home/erik/workgraph/.wg/nex-sessions/
```

But there's no environment variable or documented method to discover this location.

## Proposed Solutions

### For Immediate Improvement:
1. **Add Source Metadata**: Include original URLs and citation information in research tool outputs
2. **Environment Integration**: Create environment variables for data discovery
3. **Documentation**: Document the current file structure and access methods

### For Future Enhancement:
1. **Source Database**: Implement a persistent database of consulted sources
2. **Citation Engine**: Add proper citation formatting capabilities
3. **API Access**: Provide programmatic access to conversation data

## Recommendations

1. **Implement Environment Variables**:
   - `WG_DATA_DIR` - Root directory for wg data
   - `WG_CONVERSATION_DIR` - Directory containing conversation logs
   - `WORKGRAPH_RESEARCH_DIR` - Directory for research outputs

2. **Enhance Output Format**:
   - Include source URLs in all research outputs
   - Add citation metadata fields
   - Maintain source tracking logs

3. **Create Documentation**:
   - Document current file structure
   - Provide access patterns for research data
   - Explain how to locate conversation history

## Next Steps

1. Implement basic source tracking in research tools
2. Add environment variables for data discovery  
3. Document current implementation details
4. Plan for enhanced citation and source management features

## Conclusion

The current wg research tools provide valuable information synthesis but lack essential source tracking capabilities that are fundamental for academic and professional use cases. Addressing these limitations will significantly improve the utility of these tools.
