<?xml version="1.0" encoding="UTF-8"?>
<!-- Browser-only libvirt overlay for hosts without a usable GL render node. -->
<xsl:stylesheet version="1.0"
  xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="xml" omit-xml-declaration="yes"/>
  <xsl:template match="@*|node()">
    <xsl:copy>
      <xsl:apply-templates select="@*|node()"/>
    </xsl:copy>
  </xsl:template>
  <xsl:template match="/domain/devices">
    <xsl:copy>
      <xsl:apply-templates select="@*|node()"/>
      <sound model="virtio"/>
      <audio id="1" type="pulseaudio" serverName="tcp:127.0.0.1:4713">
        <input name="browser-vm-capture"/>
        <output name="browser-vm" streamName="MCNF-Browser-VM"/>
      </audio>
      <channel type="spicevmc">
        <target type="virtio" name="com.redhat.spice.0"/>
      </channel>
      <!-- 2D virtio is the bootable compatibility path; acceleration is opt-in
           only after the host capability preflight proves GL support. -->
      <video>
        <model type="virtio" heads="1" primary="yes"/>
      </video>
    </xsl:copy>
  </xsl:template>
</xsl:stylesheet>
