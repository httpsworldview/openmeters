import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window
import org.kde.kirigami as Kirigami

Kirigami.ApplicationWindow {
    id: root
    visible: true
    width: 720
    height: 480
    minimumWidth: 480
    minimumHeight: 320
    title: qsTr("OpenMeters")

    Rectangle {
        anchors.fill: parent
        color: "#202124"

        ColumnLayout {
            anchors.centerIn: parent
            spacing: 12

            Label {
                Layout.alignment: Qt.AlignHCenter
                text: qsTr("OpenMeters")
                font.pixelSize: 32
                color: "#e8eaed"
            }

            Label {
                Layout.alignment: Qt.AlignHCenter
                text: qsTr("Audio telemetry UI coming soon…")
                font.pixelSize: 16
                color: "#bdc1c6"
            }
        }
    }
}