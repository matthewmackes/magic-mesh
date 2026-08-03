<?xml version="1.0" encoding="UTF-8"?>
<!-- Browser-only libvirt overlay: keep the generic VM module media-neutral. -->
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
      <!-- The guest spice-vdagentd uses this channel to negotiate monitor
           geometry before wlroots commits the virtio-gpu scanout. -->
      <channel type="spicevmc">
        <target type="virtio" name="com.redhat.spice.0"/>
      </channel>
      <video>
        <model type="virtio" heads="1" primary="yes">
          <acceleration accel3d="yes"/>
        </model>
      </video>
    </xsl:copy>
  </xsl:template>
  <xsl:template match="/domain/devices/graphics[@type='spice']">
    <xsl:copy>
      <xsl:apply-templates select="@*|node()"/>
      <!-- Let libvirt select the host's render node; renderD128 is not a
           stable identity across the Dell, farm, and future seats. -->
      <gl enable="yes"/>
    </xsl:copy>
  </xsl:template>
</xsl:stylesheet>
